#include "rinzu/source_async.h"
#include "rinzu/managed_errors.h"
#include "rinzu/native_opaque.h"
#include "rinzu/continuation.h"
#include <errno.h>
#include <limits.h>
#include <stddef.h>
#include <time.h>

/* Private AOT source runtime; native physical entry points use their own ABI. */
enum {
  RINZU_EVENT_TASK_START,
  RINZU_EVENT_TASK_SUCCESS,
  RINZU_EVENT_TASK_FAILURE,
  RINZU_EVENT_TASK_CANCELLED,
  RINZU_EVENT_AWAIT_WORKER,
  RINZU_EVENT_AWAIT_FILE,
  RINZU_EVENT_AWAIT_TIMER,
  RINZU_EVENT_AWAIT_ASYNC,
  RINZU_EVENT_AWAIT_READY,
  RINZU_EVENT_NATIVE_ENTER,
  RINZU_EVENT_NATIVE_LEAVE,
  RINZU_EVENT_CALLBACK_ENTER,
  RINZU_EVENT_CALLBACK_LEAVE,
  RINZU_EVENT_AWAIT_SUPPLANT
};
typedef struct RinzuAsyncEvent {
  unsigned kind;
  uint64_t future, scope;
  struct RinzuAsyncEvent *next;
} RinzuAsyncEvent;
typedef struct RinzuFutureAdmission {
  uint64_t future, parent_task, ordinal;
  struct RinzuFutureAdmission *next;
} RinzuFutureAdmission;
typedef struct RinzuAwaitedValue {
  unsigned kind;
  uint64_t bits, scope, future;
  struct RinzuAwaitedValue *next;
} RinzuAwaitedValue;
typedef struct RinzuAsyncResultRoot {
  /* Optional handle: zero is a strong physical-return pin, not Future ID 0. */
  uint64_t future_handle;
  rinzu_managed_roots_t roots;
  rinzu_managed_root_set_t entry;
  struct RinzuAsyncResultRoot *next;
} RinzuAsyncResultRoot;
typedef struct RinzuTaskScope {
  uint64_t identity, depth, cleanup_region;
  rinzu_child_scope_t body, cleanup;
  rinzu_child_scope_t *parent;
  int cleaning, allow_cancel;
  struct RinzuTaskScope *previous;
} RinzuTaskScope;
#ifdef RINZU_AOT_HAS_SUPPLANT
typedef struct RinzuSupplant RinzuSupplant;
static int rinzu_aot_supplant_drive(RinzuContext *);
static int rinzu_aot_supplant_cancel(RinzuContext *, rinzu_async_future_t *);
#endif
typedef struct RinzuCompletionTurn {
  uint64_t identity;
  RinzuAsyncTask *task;
  rinzu_child_scope_t *scope;
  rinzu_ready_work_t *ready;
  struct RinzuCompletionTurn *next, *previous;
  int settled;
} RinzuCompletionTurn;
struct RinzuAsyncTask {
  RinzuContext context;
  rinzu_continuation_t continuation;
  rinzu_child_scope_t children;
  rinzu_async_future_t *future;
  rinzu_async_future_t *waiting;
  RinzuCompletionTurn completion_turn;
  int waiting_for_completion;
  rinzu_async_ready_waiter_t readiness;
  rinzu_wait_dependency_t *wait_dependency;
  rinzu_source_position_t function_source;
  const char *function_label;
  int64_t index;
  uint64_t *arguments;
  size_t count;
  size_t result_type;
  RinzuAsyncResultRoot *result_root;
  struct RinzuAsyncTask *next, *previous;
  struct RinzuAsyncTask *ready_next, *ready_previous;
  int ready_queued;
  /* This bottom frame and its typed root slots have exactly the task lifetime.
     Completed Future result roots remain independently allocated. */
  rinzu_managed_frame_t argument_frame;
  uint64_t argument_storage[];
};
static RinzuAsyncTask *rinzu_aot_async_allocate_task(rinzu_async_runtime_t *runtime,
                                                     const uint64_t *arguments, size_t count) {
  if ((count && !arguments) || count > (SIZE_MAX - sizeof(RinzuAsyncTask)) / (2 * sizeof(uint64_t)))
    rinzu_failure_fatal();
  RinzuAsyncTask *task =
      rinzu_async_control_allocate(runtime, sizeof(*task) + count * 2 * sizeof(uint64_t));
  task->count = count;
  task->argument_frame.allocation = 1;
  if (count) {
    task->arguments = task->argument_storage;
    memcpy(task->arguments, arguments, count * sizeof(uint64_t));
    task->argument_frame.roots = task->argument_storage + count;
    task->argument_frame.capacity = count;
  }
  /* prepare publishes at most one typed root per argument, before the body
     enters a separate frame. This frame can never need a larger buffer. */
  task->context.roots.free_frames = &task->argument_frame;
  return task;
}
static void rinzu_aot_async_detach_argument_frame(RinzuAsyncTask *task) {
  rinzu_managed_roots_t *roots = &task->context.roots;
  /* The body pops its frame, then its admission frame. Consequently this is
     the head of the inactive chain, including when the body failed/cancelled. */
  if (roots->current || roots->depth || roots->free_frames != &task->argument_frame ||
      task->argument_frame.count || task->argument_frame.writing ||
      task->argument_frame.allocation != 1 || task->argument_frame.capacity != task->count ||
      task->argument_frame.roots != (task->count ? task->argument_storage + task->count : NULL))
    rinzu_failure_fatal();
  roots->free_frames = task->argument_frame.previous;
}
struct RinzuAsyncState {
  rinzu_async_runtime_t runtime;
  rinzu_async_future_t *retired_evidence;
  rinzu_child_scope_t children;
  const char *executable;
  RinzuAsyncEvent *events, *events_tail;
  RinzuFutureAdmission *admissions, *admissions_tail;
  RinzuAwaitedValue *values, *values_tail;
  RinzuContext *root;
  RinzuAsyncTask *tasks;
  RinzuAsyncTask *ready_head, *ready_tail;
  RinzuCompletionTurn *completion_head, *completion_tail;
  size_t ready_count;
  unsigned ready_order;
  RinzuAsyncResultRoot *result_roots;
  size_t next_future_gc;
  size_t peak_future_records, gc_retired_futures;
  rinzu_continuation_t root_continuation;
  rinzu_activity_t *root_activity;
  rinzu_source_position_t root_source;
  const char *root_label;
  rinzu_continuation_accounting_t continuations;
  uint64_t polls;
  size_t started_workers, started_files, started_timers, started_native_completions;
  int first_await_seen, explicit_shutdown;
  int exit_status;
  size_t first_await_workers, first_await_files;
#ifdef RINZU_AOT_HAS_SUPPLANT
  rinzu_supplant_t supplant_gate;
  RinzuSupplant *supplants, *active_supplant;
  rinzu_foreign_queue_t *foreign_queue;
  rinzu_owner_dispatcher_t *dispatcher;
#endif
};
enum { RINZU_TASK_READY_ASCENDING = 1, RINZU_TASK_READY_DESCENDING = 2 };
static int rinzu_aot_async_runnable(const RinzuAsyncTask *task) {
  return !task->continuation.running &&
         (!task->continuation.started || (task->waiting && task->waiting->settled) ||
          (task->waiting_for_completion && task->completion_turn.settled));
}
/* Owner-only readiness edges. Keep the lifetime list separate; no queue node,
   lock or allocation is needed. Monotonic arrival runs need no later sort. */
static void rinzu_aot_async_queue_ready(RinzuAsyncTask *task) {
  RinzuAsyncState *state = task->context.async;
  if (task->ready_queued || task->ready_next || task->ready_previous ||
      state->ready_count == SIZE_MAX || !rinzu_aot_async_runnable(task))
    rinzu_failure_fatal();
  if (state->ready_tail) {
    if (state->ready_order)
      state->ready_order &= state->ready_tail->future->identity < task->future->identity
                                ? RINZU_TASK_READY_ASCENDING
                                : RINZU_TASK_READY_DESCENDING;
    state->ready_tail->ready_next = task;
  } else {
    if (state->ready_head || state->ready_count)
      rinzu_failure_fatal();
    state->ready_head = task;
    state->ready_order = RINZU_TASK_READY_ASCENDING | RINZU_TASK_READY_DESCENDING;
  }
  task->ready_previous = state->ready_tail;
  state->ready_tail = task;
  task->ready_queued = 1;
  ++state->ready_count;
}
static void rinzu_aot_async_unqueue_ready(RinzuAsyncTask *task) {
  if (!task->ready_queued) {
    if (task->ready_next || task->ready_previous)
      rinzu_failure_fatal();
    return; /* A control resume may interrupt a still-pending await. */
  }
  RinzuAsyncState *state = task->context.async;
  if (!state->ready_count ||
      (task->ready_previous ? task->ready_previous->ready_next != task
                            : state->ready_head != task) ||
      (task->ready_next ? task->ready_next->ready_previous != task : state->ready_tail != task))
    rinzu_failure_fatal();
  if (task->ready_previous)
    task->ready_previous->ready_next = task->ready_next;
  else
    state->ready_head = task->ready_next;
  if (task->ready_next)
    task->ready_next->ready_previous = task->ready_previous;
  else
    state->ready_tail = task->ready_previous;
  task->ready_next = task->ready_previous = NULL;
  task->ready_queued = 0;
  /* Removing from a monotonic run preserves its direction. An unknown order
     stays conservative until only one entry remains; never scan to repair it. */
  if (--state->ready_count < 2)
    state->ready_order = RINZU_TASK_READY_ASCENDING | RINZU_TASK_READY_DESCENDING;
}
static void rinzu_aot_async_waiter_ready(rinzu_async_ready_waiter_t *waiter) {
  RinzuAsyncTask *task =
      (RinzuAsyncTask *)((unsigned char *)waiter - offsetof(RinzuAsyncTask, readiness));
  if (task->waiting != waiter->future)
    rinzu_failure_fatal();
  rinzu_aot_async_queue_ready(task);
}
static void rinzu_aot_async_event(void *raw, unsigned kind, uint64_t future) {
  RinzuAsyncState *state = ((RinzuContext *)raw)->async;
  if (!state || !state->runtime.record_evidence)
    return;
  rinzu_async_owner(&state->runtime);
  RinzuAsyncEvent *event = rinzu_failure_allocate(sizeof(*event));
  event->kind = kind;
  event->future = future;
  RinzuContext *context = raw;
  event->scope = context->task ? context->task->future->identity + 1 : 0;
  if (state->events_tail)
    state->events_tail->next = event;
  else
    state->events = event;
  state->events_tail = event;
}
#define ASYNC_EVENT(raw, kind, future) rinzu_aot_async_event(raw, kind, future)
static void rinzu_aot_async_admission(rinzu_async_future_t *future, void *raw) {
  RinzuAsyncState *state = raw;
  if (state->runtime.records > state->peak_future_records)
    state->peak_future_records = state->runtime.records;
  if (!state->runtime.record_evidence)
    return;
  RinzuFutureAdmission *entry = rinzu_failure_allocate(sizeof *entry);
  entry->future = future->identity;
  entry->parent_task = future->parent_task;
  entry->ordinal = future->ordinal;
  if (state->admissions_tail)
    state->admissions_tail->next = entry;
  else
    state->admissions = entry;
  state->admissions_tail = entry;
}
static RinzuAsyncState *rinzu_aot_async_state(RinzuContext *context) {
  if (!context->async)
    rinzu_failure_fatal();
  rinzu_async_owner(&context->async->runtime);
  return context->async;
}
#ifdef RINZU_AOT_HAS_SOURCE_NATIVE
static void *rinzu_aot_callback_parent_scope(RinzuContext *context) {
  return context->async ? rinzu_aot_async_state(context)->runtime.scope : NULL;
}
static void *rinzu_aot_callback_enter_scope(RinzuContext *context, void *scope) {
  if (!context->async) {
    if (scope)
      _Exit(86);
    return NULL;
  }
  RinzuAsyncState *state = rinzu_aot_async_state(context);
  void *previous = state->runtime.scope;
  state->runtime.scope = scope;
  if (state->runtime.callback_admission == UINT_MAX)
    rinzu_failure_fatal();
  state->runtime.callback_admission += 1;
  return previous;
}
static void rinzu_aot_callback_leave_scope(RinzuContext *context, void *scope) {
  if (!context->async) {
    if (scope)
      _Exit(86);
    return;
  }
  RinzuAsyncState *state = rinzu_aot_async_state(context);
  if (!state->runtime.callback_admission)
    rinzu_failure_fatal();
  state->runtime.callback_admission -= 1;
  state->runtime.scope = scope;
}
#endif
static int rinzu_aot_async_cancelled(RinzuContext *context) {
  return context->cancel_requested ||
         (context->worker &&
          atomic_load_explicit(&context->worker->cancel_requested, memory_order_acquire));
}
static void rinzu_aot_async_credit_evidence(RinzuAsyncState *state) {
  if (!state->runtime.record_evidence)
    return;
  printf("RINZU_CREDITS=");
  const char *separator = "";
  for (rinzu_async_credit_t *credit = state->runtime.credits; credit; credit = credit->next) {
    printf("%s%llu:%llu:%u:%zu:%llu:%llu", separator, (unsigned long long)credit->reserved_at,
           (unsigned long long)credit->operation, credit->direction, credit->bytes,
           (unsigned long long)credit->reserved_at, (unsigned long long)credit->released_at);
    separator = ";";
  }
  printf("\nRINZU_CANCELLED_CHILDREN=");
  separator = "";
  for (unsigned group = 0; group < 2; ++group)
    for (rinzu_async_future_t *future = group ? state->retired_evidence : state->runtime.futures;
         future; future = future->next) {
      if (atomic_load_explicit(&future->cancel_requested, memory_order_acquire)) {
        printf("%s%llu", separator, (unsigned long long)future->identity);
        separator = ",";
      }
    }
  printf("\n");
}
/* Each catch keeps its local Error snapshot. The Main report records the
   observation immediately, including catches made by a synchronous helper or
   a callee prefix before that callee suspends. */
static void rinzu_aot_async_observe(RinzuContext *context, const rinzu_failure_t *failure) {
  if (!context->async || context == context->async->root)
    return;
  rinzu_failure_state_t *report = &context->async->root->pending;
  if (report->observed_count == INT64_MAX)
    rinzu_failure_fatal();
  rinzu_observed_error_t *error = rinzu_failure_allocate(sizeof(*error));
  error->failure = rinzu_failure_copy(failure);
  rinzu_failure_forget_managed(&error->failure);
  error->identity = ++report->observed_count;
  error->next = report->observed;
  report->observed = error;
}
static rinzu_async_future_t *rinzu_aot_worker_request(RinzuContext *, int64_t, const uint64_t *,
                                                      int64_t);
/* A rejected reservation is never exposed as a Future. Settle its private
   resources, but route the control failure through this call's context. */
static int64_t rinzu_aot_async_reject(RinzuContext *context, rinzu_async_future_t *future) {
  RinzuAsyncState *state = context->async;
  if (future->started || future->settled || !future->failure.primary)
    rinzu_failure_fatal();
  if (future->scope && rinzu_child_withdraw_admission(future->scope, future->identity))
    rinzu_failure_fatal();
  future->scope = NULL;
  rinzu_async_materialize(future);
  if (context->pending.current.primary)
    rinzu_failure_fatal();
  context->pending.current = rinzu_failure_copy(&future->failure);
  RinzuFutureAdmission **link = &state->admissions;
  RinzuFutureAdmission *previous = NULL;
  while (*link && (*link)->future != future->identity) {
    previous = *link;
    link = &(*link)->next;
  }
  if (*link) {
    RinzuFutureAdmission *entry = *link;
    *link = entry->next;
    if (state->admissions_tail == entry)
      state->admissions_tail = previous;
    free(entry);
  }
  rinzu_async_release(future);
  return 0;
}
int64_t rinzu_baseline_async_spawn(void *raw, int64_t index, const uint64_t *arguments,
                                   int64_t count) {
  RinzuContext *context = raw;
  rinzu_aot_async_state(context);
  rinzu_async_future_t *future = rinzu_aot_worker_request(context, index, arguments, count);
  if (!rinzu_async_admit(future)) {
    future->failure.primary = 3;
    future->failure.primary_source = context->source;
    return rinzu_aot_async_reject(context, future);
  } else {
    rinzu_async_start(future);
    if (future->started)
      context->async->started_workers += 1;
    else
      return rinzu_aot_async_reject(context, future);
  }
  return (int64_t)rinzu_async_handle(future);
}
static void rinzu_aot_async_close_scope(RinzuContext *, rinzu_child_scope_t *, int);
static void rinzu_aot_async_prepare(RinzuAsyncTask *);
static int64_t rinzu_aot_async_entry(RinzuAsyncTask *);
void rinzu_baseline_leave_dispatch(void *);
static RinzuAsyncTask *rinzu_aot_async_task(RinzuAsyncState *state,
                                            const rinzu_async_future_t *future) {
  rinzu_async_owner(&state->runtime);
  RinzuAsyncTask *task = future->task_context;
  if (future->runtime != &state->runtime || (task && task->future != future))
    rinzu_failure_fatal();
  return task;
}
static void rinzu_aot_async_release_result(RinzuAsyncState *state, RinzuAsyncResultRoot *root) {
  RinzuAsyncResultRoot **cursor = &state->result_roots;
  while (*cursor && *cursor != root)
    cursor = &(*cursor)->next;
  if (!*cursor)
    rinzu_failure_fatal();
  *cursor = root->next;
  rinzu_async_future_t *future = rinzu_async_lookup(&state->runtime, root->future_handle);
  if (future)
    future->gc_result = 0;
  rinzu_managed_leave(&root->roots);
  rinzu_managed_detach_roots(state->root->shared_data, &root->entry);
  rinzu_managed_dispose(&root->roots);
  free(root);
}
static uint64_t rinzu_aot_future_mark(void *raw, uint64_t handle) {
  RinzuAsyncState *state = raw;
  rinzu_async_future_t *future = rinzu_async_lookup(&state->runtime, handle);
  if (future) {
    if (future->gc_reached)
      return 0;
    future->gc_reached = 1;
    return future->gc_result;
  }
  /* A direct consuming await may leave a stale, statically dead root word. */
  return 0;
}
static void rinzu_aot_future_begin(void *raw) {
  RinzuAsyncState *state = raw;
  for (rinzu_async_future_t *future = state->runtime.futures; future; future = future->next)
    future->gc_reached = 0;
  for (rinzu_async_future_t *future = state->runtime.futures; future; future = future->next)
    if (!future->settled || future->ready_waiters) {
      future->gc_reached = 1;
      if (future->gc_result)
        rinzu_managed_mark(state->root->shared_data, future->gc_result);
    }
}
static void rinzu_aot_future_evidence(RinzuAsyncState *state, const rinzu_async_future_t *future) {
  int cancelled = atomic_load_explicit(&future->cancel_requested, memory_order_acquire);
  if (!state->runtime.record_evidence || (!future->heap_evidence && !cancelled))
    return;
  /* Zero-owned scalar snapshot: never a task, Future, packet, root or resource. */
  rinzu_async_future_t *snapshot = rinzu_failure_allocate(sizeof(*snapshot));
  snapshot->identity = future->identity;
  snapshot->heap_evidence = future->heap_evidence;
  snapshot->worker_body_entered = future->worker_body_entered;
  snapshot->worker_objects = future->worker_objects;
  snapshot->worker_roots = future->worker_roots;
  snapshot->worker_peak_objects = future->worker_peak_objects;
  snapshot->worker_peak_frames = future->worker_peak_frames;
  snapshot->worker_collections = future->worker_collections;
  snapshot->worker_userdata_scopes = future->worker_userdata_scopes;
  snapshot->worker_userdata_tokens = future->worker_userdata_tokens;
  snapshot->worker_userdata_roots = future->worker_userdata_roots;
  snapshot->worker_userdata_in_flight = future->worker_userdata_in_flight;
  snapshot->worker_userdata_created = future->worker_userdata_created;
  snapshot->worker_userdata_released = future->worker_userdata_released;
  atomic_init(&snapshot->cancel_requested, cancelled);
  snapshot->next = state->retired_evidence;
  state->retired_evidence = snapshot;
}
static void rinzu_aot_future_sweep(void *raw) {
  RinzuAsyncState *state = raw;
  rinzu_async_future_t *future = state->runtime.futures;
  while (future) {
    rinzu_async_future_t *next = future->next;
    if (!future->gc_reached && future->settled && !future->ready_waiters &&
        (!future->scope || future->scope->finished ||
         rinzu_child_retire_unreferenced(future->scope, future->identity))) {
      for (RinzuAsyncResultRoot *root = state->result_roots; root; root = root->next)
        if (root->future_handle == rinzu_async_handle(future)) {
          rinzu_aot_async_release_result(state, root);
          break;
        }
      rinzu_aot_future_evidence(state, future);
      rinzu_async_release(future);
      ++state->gc_retired_futures;
    }
    future = next;
  }
  size_t records = state->runtime.records;
  state->next_future_gc = records > SIZE_MAX / 2 ? SIZE_MAX : records * 2;
  if (state->next_future_gc < 1024)
    state->next_future_gc = 1024;
}
static int rinzu_aot_future_due(void *raw) {
  RinzuAsyncState *state = raw;
  return state->runtime.records >= (state->next_future_gc ? state->next_future_gc : 1024);
}
static void rinzu_aot_async_hold_result(RinzuAsyncTask *task, uint64_t slot) {
  RinzuAsyncState *state = task->context.async;
  rinzu_managed_data_t *data = rinzu_context_data(&task->context);
  rinzu_managed_slot_t value = {slot, task->future->result_kind == RINZU_ASYNC_MANAGED};
  if (rinzu_managed_slot_valid(data, task->result_type, value))
    rinzu_failure_fatal();
  if (task->future->result_kind == RINZU_ASYNC_FUTURE)
    (void)rinzu_async_find(&state->runtime, slot);
  if (!value.reference)
    return;
  RinzuAsyncResultRoot *root = rinzu_failure_allocate(sizeof *root);
  root->future_handle = rinzu_async_handle(task->future);
  rinzu_managed_attach_roots(data, &root->entry, &root->roots);
  root->entry.future_owner = rinzu_async_handle(task->future);
  task->future->gc_result = slot;
  rinzu_managed_enter(&root->roots);
  rinzu_managed_publish(&root->roots, &slot, 1);
  root->next = state->result_roots;
  state->result_roots = root;
  task->result_root = root;
}
/* Child records already identify the Futures owned by this scope. Update
   before handoff/finish_close move or free those records; no other Future can
   acquire this scope on the Owner while this non-suspending loop runs. */
static void rinzu_aot_async_reparent_scope(RinzuAsyncState *state, rinzu_child_scope_t *scope,
                                           rinzu_child_scope_t *target) {
  for (rinzu_child_t *child = scope->children; child; child = child->next) {
    rinzu_async_future_t *future =
        rinzu_async_lookup(&state->runtime, RINZU_ASYNC_HANDLE_BASE + child->identity + 1);
    if (future && future->scope == scope)
      future->scope = target;
  }
}
static void rinzu_aot_async_body(void *raw) {
  RinzuAsyncTask *task = raw;
  RinzuContext *context = &task->context;
  RinzuAsyncState *state = rinzu_aot_async_state(context);
  uint64_t slot = (uint64_t)rinzu_aot_async_entry(task);
  if (!context->pending.current.primary)
    rinzu_aot_async_hold_result(task, slot);
  rinzu_baseline_leave_dispatch(context);
  rinzu_failure_state_t completed_body = {.current = context->pending.current};
  if (!context->pending.current.primary || rinzu_failure_recoverable(&completed_body)) {
    rinzu_aot_async_reparent_scope(state, &task->children, &state->children);
    if (rinzu_child_handoff(&task->children, &state->children))
      rinzu_failure_fatal();
  }
  rinzu_aot_async_close_scope(context, &task->children, !context->cancel_masked);
  task->future->failure = context->pending.current;
  context->pending.current = (rinzu_failure_t){0};
  task->future->result_slot = slot;
  rinzu_async_materialize(task->future);
  if (task->future->failure.primary && task->result_root) {
    rinzu_aot_async_release_result(state, task->result_root);
    task->result_root = NULL;
  }
  rinzu_child_dispose(&task->children);
  /* The prepared argument frame is separate from the compiled body frame. */
  rinzu_managed_leave(&context->roots);
}
static void rinzu_aot_async_resume(RinzuContext *caller, RinzuAsyncTask *task) {
  RinzuAsyncState *state = rinzu_aot_async_state(caller);
  rinzu_aot_async_unqueue_ready(task);
  if (task->wait_dependency) {
    rinzu_activity_wait_end(task->wait_dependency);
    task->wait_dependency = NULL;
  }
  if (task->readiness.future)
    rinzu_async_waiter_receive(&task->readiness);
  rinzu_owner_note_turn(rinzu_context_transitions(&task->context));
  rinzu_child_scope_t *scope = state->runtime.scope;
  RinzuTaskScope *group = task->context.task_scopes;
  state->runtime.scope =
      group ? (group->cleaning ? &group->cleanup : &group->body) : &task->children;
  rinzu_continuation_t *continuation =
      caller->task ? &caller->task->continuation : &state->root_continuation;
  rinzu_activity_t *activity = rinzu_owner_activity_prepare(
      rinzu_context_transitions(&task->context), RINZU_ACTIVITY_MANAGED, task->function_label,
      task->function_source.file, task->function_source.start_byte, task->function_source.end_byte,
      task->context.roots.depth);
  rinzu_activity_start(activity);
  rinzu_continuation_resume(continuation, &task->continuation);
  rinzu_activity_end(activity);
  state->runtime.scope = scope;
  if (task->continuation.finished) {
    if (!task->future->settled || task->waiting || task->completion_turn.task ||
        task->waiting_for_completion || task->context.roots.depth || task->context.return_roots ||
        task->context.error_groups.depth)
      rinzu_failure_fatal();
    if ((task->previous ? task->previous->next != task : state->tasks != task) ||
        (task->next && task->next->previous != task) || task->future->task_context != task)
      rinzu_failure_fatal();
    if (task->previous)
      task->previous->next = task->next;
    else
      state->tasks = task->next;
    if (task->next)
      task->next->previous = task->previous;
    task->future->task_context = NULL;
    rinzu_managed_detach_roots(task->context.shared_data, &task->context.domain_roots);
    rinzu_aot_async_detach_argument_frame(task);
    rinzu_managed_dispose(&task->context.roots);
    rinzu_failure_dispose(&task->context.pending);
    rinzu_continuation_dispose(&task->continuation);
    rinzu_async_control_release(&state->runtime, task,
                                sizeof(*task) + task->count * 2 * sizeof(uint64_t));
  }
}
int64_t rinzu_baseline_async_call(void *raw, int64_t index, const uint64_t *arguments,
                                  int64_t count, int64_t kind) {
  RinzuContext *context = raw;
  RinzuAsyncState *state = rinzu_aot_async_state(context);
  if (count < 0 || (uint64_t)count > SIZE_MAX / sizeof(uint64_t) || (count && !arguments))
    rinzu_failure_fatal();
  /* Admission roots the environment; the body is selected by a runtime turn. */
  if (!context->cancel_masked && !context->pending.depth &&
      state->runtime.live_main_tasks >= state->runtime.main_task_limit) {
    rinzu_failure_fail(&context->pending, 3, context->source);
    return 0;
  }
  rinzu_async_future_t *future =
      rinzu_async_new(&state->runtime, RINZU_ASYNC_TASK, (unsigned)kind, context->source);
  ++state->runtime.live_main_tasks;
  future->main_admitted = 1;
  RinzuAsyncTask *task = rinzu_aot_async_allocate_task(&state->runtime, arguments, (size_t)count);
  task->future = future;
  task->index = index;
  task->context.async = state;
  task->context.task = task;
  task->context.shared_data = rinzu_context_data(context);
  task->context.domain_polls = &state->polls;
  task->context.shared_userdata = rinzu_context_userdata(context);
  task->context.shared_transitions = rinzu_context_transitions(context);
  task->context.foreign_queue = context->foreign_queue;
  task->context.cancel_after = context->cancel_after;
  task->context.shutdown_after = context->shutdown_after;
  task->context.cancel_masked =
      context->cancel_masked || context->pending.depth || context->callback_depth;
  rinzu_child_init(&task->children);
  task->children.task_identity = future->identity + 1;
  rinzu_managed_attach_roots(task->context.shared_data, &task->context.domain_roots,
                             &task->context.roots);
  rinzu_managed_enter(&task->context.roots);
  rinzu_aot_async_prepare(task);
  task->next = state->tasks;
  if (task->next)
    task->next->previous = task;
  state->tasks = task;
  future->task_context = task;
  rinzu_continuation_init(&task->continuation, 256 * 1024, rinzu_aot_async_body, task,
                          &state->continuations);
  rinzu_aot_async_queue_ready(task);
  return (int64_t)rinzu_async_handle(future);
}
#ifdef RINZU_AOT_HAS_FILE_READ
static void rinzu_aot_read_file(rinzu_async_future_t *future) {
  errno = 0;
  FILE *file = fopen((const char *)future->native_payload, "rb");
  if (!file && errno == ENOMEM)
    rinzu_failure_fatal();
  int failed = !file;
  uint64_t size = 0;
  if (file) {
    while (fgetc(file) != EOF) {
      if (size == INT64_MAX) {
        failed = 1;
        break;
      }
      size += 1;
    }
    if (ferror(file)) {
      if (errno == ENOMEM)
        rinzu_failure_fatal();
      failed = 1;
    }
    if (fclose(file)) {
      if (errno == ENOMEM)
        rinzu_failure_fatal();
      failed = 1;
    }
  }
  if (failed) {
    future->failure.primary = 12;
    future->failure.primary_source = future->source;
  } else
    future->result_slot = size;
}
static int64_t rinzu_baseline_async_file(void *raw, int64_t string) {
  RinzuContext *context = raw;
  RinzuAsyncState *state = rinzu_aot_async_state(context);
  rinzu_managed_value_t *path =
      rinzu_managed_string_value(rinzu_context_data(context), (uint64_t)string);
  /* Match the JIT package-relative service boundary. Keep an owned native
     copy; the I/O thread never reads a String or any Origin heap object. */
  int invalid = !path->length || path->bytes[0] == '/' || memchr(path->bytes, 0, path->length);
  size_t start = 0;
  for (size_t index = 0; index <= path->length; ++index) {
    if (index == path->length || path->bytes[index] == '/') {
      if (index - start == 2 && path->bytes[start] == '.' && path->bytes[start + 1] == '.')
        invalid = 1;
      if (start == 0 && index - start == 1 && path->bytes[start] == '.')
        invalid = 1;
      start = index + 1;
    }
  }
  if (invalid) {
    rinzu_failure_fail(&context->pending, 12, context->source);
    return 0;
  }
  rinzu_async_future_t *future =
      rinzu_async_new(&state->runtime, RINZU_ASYNC_FILE, RINZU_SCALAR_I64, context->source);
  future->run = rinzu_aot_read_file;
  size_t prefix = strlen(state->executable);
  static const char separator[] = ".resources/";
  if (prefix > SIZE_MAX - sizeof(separator) || path->length > SIZE_MAX - prefix - sizeof(separator))
    rinzu_failure_fatal();
  future->native_payload = rinzu_failure_allocate(prefix + sizeof(separator) + path->length);
  memcpy(future->native_payload, state->executable, prefix);
  memcpy(future->native_payload + prefix, separator, sizeof(separator) - 1);
  memcpy(future->native_payload + prefix + sizeof(separator) - 1, path->bytes, path->length);
  {
    rinzu_async_set_readiness(
        future, rinzu_owner_ready_prepare(rinzu_context_transitions(context),
                                          "reactor.file-read-size", future->source.file,
                                          future->source.start_byte, future->source.end_byte));
    rinzu_async_start(future);
    if (future->started)
      state->started_files += 1;
    else
      return rinzu_aot_async_reject(context, future);
  }
  return (int64_t)rinzu_async_handle(future);
}
#endif
#ifdef RINZU_AOT_HAS_TIMER
static void rinzu_aot_wait_timer(rinzu_async_future_t *future) {
  int64_t milliseconds;
  memcpy(&milliseconds, future->native_payload, sizeof(milliseconds));
  if (milliseconds < 0)
    rinzu_failure_fatal();
  struct timespec remaining = {(time_t)(milliseconds / 1000),
                               (long)(milliseconds % 1000) * 1000000};
  while (nanosleep(&remaining, &remaining)) {
    if (errno != EINTR)
      rinzu_failure_fatal();
  }
  /* Unit has no managed payload. The common helper publishes physical_done,
     readiness and a wake only after the actual deadline/return. */
}
static int64_t rinzu_baseline_async_timer(void *raw, int64_t milliseconds) {
  RinzuContext *context = raw;
  RinzuAsyncState *state = rinzu_aot_async_state(context);
  if (milliseconds < 0) {
    rinzu_failure_fail(&context->pending, 5, context->source);
    return 0;
  }
  rinzu_async_future_t *future =
      rinzu_async_new(&state->runtime, RINZU_ASYNC_TIMER, RINZU_TRANSFER_UNIT, context->source);
  {
    future->run = rinzu_aot_wait_timer;
    future->native_payload = rinzu_failure_allocate(sizeof(milliseconds));
    memcpy(future->native_payload, &milliseconds, sizeof(milliseconds));
    rinzu_async_set_readiness(
        future, rinzu_owner_ready_prepare(rinzu_context_transitions(context), "reactor.timer",
                                          future->source.file, future->source.start_byte,
                                          future->source.end_byte));
    // Even zero milliseconds stays pending until an Owner turn collects the
    // helper's actual return. It never completes in this synchronous prefix.
    rinzu_async_start(future);
    if (future->started)
      state->started_timers += 1;
    else
      return rinzu_aot_async_reject(context, future);
  }
  return (int64_t)rinzu_async_handle(future);
}
#endif
int64_t rinzu_baseline_async_service(void *raw, int64_t service, int64_t argument) {
  if (service == 3 || service == 4 || service == 5) {
    RinzuContext *context = raw;
    RinzuAsyncState *state = rinzu_aot_async_state(context);
    rinzu_managed_data_t *data = rinzu_context_data(context);
    rinzu_managed_value_t *value = rinzu_managed_find(data, (uint64_t)argument);
    rinzu_native_completion_t *completion =
        rinzu_managed_opaque_pointer(data, (uint64_t)argument, value->type);
    rinzu_async_runtime_t *runtime = &state->runtime;
    if (service == 5 && context->task && !context->task->completion_turn.task && runtime->scope &&
        runtime->direct_native_completion && !runtime->record_evidence && !context->cancel_after &&
        !context->shutdown_after && !context->cancel_requested && !context->shutdown_requested &&
        !state->explicit_shutdown && !context->pending.current.primary &&
        rinzu_native_completion_valid(completion) && rinzu_native_completion_is_done(completion)) {
      /* The only consumer is the adjacent await. Keep the completion's place
         among physical completions, but publish no Future or SDK pointer. */
      rinzu_child_scope_t *scope = runtime->scope;
      if (runtime->reactor_stopping || runtime->next_identity >= RINZU_ASYNC_HANDLE_BASE - 1 ||
          scope->finished || (scope->closing && !runtime->callback_admission))
        rinzu_failure_fatal();
      rinzu_async_native_progress(runtime);
      return -(int64_t)(++runtime->next_identity);
    }
    rinzu_async_future_t *future =
        rinzu_async_new(runtime, RINZU_ASYNC_COMPLETION,
                        service == 4 ? RINZU_SCALAR_I64 : RINZU_TRANSFER_UNIT, context->source);
    future->native_completion = completion;
    if (service == 4 && (future->native_completion->version != 3 ||
                         !rinzu_native_completion_valid(future->native_completion)))
      rinzu_failure_fatal();
    rinzu_async_set_readiness(
        future, rinzu_owner_ready_prepare(
                    rinzu_context_transitions(context),
                    service == 4 ? "reactor.native-transfer" : "reactor.native-completion",
                    future->source.file, future->source.start_byte, future->source.end_byte));
    rinzu_async_start(future);
    state->started_native_completions++;
    return (int64_t)rinzu_async_handle(future);
  }
#ifdef RINZU_AOT_HAS_FILE_READ
  if (service == 1)
    return rinzu_baseline_async_file(raw, argument);
#endif
#ifdef RINZU_AOT_HAS_TIMER
  if (service == 2)
    return rinzu_baseline_async_timer(raw, argument);
#endif
  (void)raw;
  (void)argument;
  (void)service;
  rinzu_failure_fatal();
  return 0;
}
static int rinzu_aot_async_compare_handles(const void *left, const void *right) {
  uint64_t a = *(const uint64_t *)left, b = *(const uint64_t *)right;
  return (a > b) - (a < b);
}
/* Keep the snapshot array off the common single-task path's stack. */
static __attribute__((noinline)) void
rinzu_aot_async_resume_snapshot(RinzuContext *context, RinzuAsyncState *state, size_t count) {
  if (count > SIZE_MAX / sizeof(uint64_t))
    rinzu_failure_fatal();
  uint64_t local_ready[64];
  uint64_t *ready = count <= sizeof(local_ready) / sizeof(*local_ready)
                        ? local_ready
                        : rinzu_failure_allocate(count * sizeof(*ready));
  size_t index = 0;
  /* Admission prepends increasing Future identities; unlinking never changes
     the lifetime list's reverse creation order. For a mixed but dense ready
     set, visiting at most twice R tasks beats sorting R handles. Sparse sets
     still use their queue, so idle tasks do not reintroduce an unbounded scan.
     live_main_tasks is maintained by admission and materialization already. */
  int creation_order = !state->ready_order && state->runtime.live_main_tasks >= count &&
                       state->runtime.live_main_tasks - count <= count;
  /* Match creation-order ready snapshots even if resuming a task makes a
     different existing task ready during this turn. */
  if (creation_order) {
    index = count;
    for (RinzuAsyncTask *task = state->tasks; task; task = task->next)
      if (task->ready_queued) {
        if (!index)
          rinzu_failure_fatal();
        ready[--index] = rinzu_async_handle(task->future);
      }
    if (index)
      rinzu_failure_fatal();
  } else if (state->ready_order & RINZU_TASK_READY_DESCENDING) {
    for (RinzuAsyncTask *task = state->ready_tail; task; task = task->ready_previous)
      ready[index++] = rinzu_async_handle(task->future);
  } else {
    for (RinzuAsyncTask *task = state->ready_head; task; task = task->ready_next)
      ready[index++] = rinzu_async_handle(task->future);
  }
  if (!creation_order && index != count)
    rinzu_failure_fatal();
  if (!creation_order && !state->ready_order)
    qsort(ready, count, sizeof *ready, rinzu_aot_async_compare_handles);
  for (index = 0; index < count; ++index) {
    rinzu_async_future_t *future = rinzu_async_lookup(&state->runtime, ready[index]);
    RinzuAsyncTask *task = future ? rinzu_aot_async_task(state, future) : NULL;
    /* A previous resume can run/cancel this task before its snapshot turn. */
    if (task && rinzu_aot_async_runnable(task))
      rinzu_aot_async_resume(context, task);
  }
  if (ready != local_ready)
    free(ready);
}
static void rinzu_aot_async_resume_ready(RinzuContext *context) {
  RinzuAsyncState *state = rinzu_aot_async_state(context);
  for (;;) {
    size_t count = state->ready_count;
    if (!count)
      return;
    if (count == 1) {
      /* No earlier resume exists in this snapshot. Owner cannot mutate the
         queue between selecting its sole task and entering resume; no handle
         lookup or repeated runnable check is needed. Resume unlinks it first. */
      rinzu_aot_async_resume(context, state->ready_head);
    } else {
      rinzu_aot_async_resume_snapshot(context, state, count);
    }
  }
}
static void rinzu_aot_async_unlink_completion_turn(RinzuAsyncState *state,
                                                   RinzuCompletionTurn *turn) {
  if (turn->previous)
    turn->previous->next = turn->next;
  else
    state->completion_head = turn->next;
  if (turn->next)
    turn->next->previous = turn->previous;
  else
    state->completion_tail = turn->previous;
}
static void rinzu_aot_async_finish_completion_turn(RinzuCompletionTurn *turn) {
  if (!turn->settled || !turn->ready)
    rinzu_failure_fatal();
  rinzu_ready_receive(turn->ready);
  rinzu_ready_dispose(turn->ready);
  *turn = (RinzuCompletionTurn){0};
}
/* The Owner drains runnable tasks before selecting one physical completion.
   These turns share the same identity order as timers, Workers and native I/O. */
static int rinzu_aot_async_collect_ready(RinzuAsyncState *state) {
  RinzuCompletionTurn *turn = state->completion_head;
  uint64_t oldest = turn ? turn->identity : UINT64_MAX;
  rinzu_async_future_t *ready = NULL;
  for (rinzu_async_future_t *future = state->runtime.futures; future; future = future->next)
    if (future->identity < oldest && !future->settled &&
        (future->operation == RINZU_ASYNC_WORKER || future->operation == RINZU_ASYNC_FILE ||
         future->operation == RINZU_ASYNC_TIMER || future->operation == RINZU_ASYNC_COMPLETION) &&
        atomic_load_explicit(&future->physical_done, memory_order_acquire)) {
      oldest = future->identity;
      ready = future;
    }
  if (ready) {
    if (!rinzu_async_try_collect(ready))
      _Exit(86);
    rinzu_async_adopt_observed(ready, &state->root->pending);
  } else if (turn) {
    rinzu_aot_async_unlink_completion_turn(state, turn);
    turn->settled = 1;
    if (turn->task->waiting_for_completion)
      rinzu_aot_async_queue_ready(turn->task);
    else
      rinzu_aot_async_finish_completion_turn(turn);
  } else {
    return 0;
  }
  return 1;
}
#ifdef RINZU_AOT_HAS_SUPPLANT
static RinzuAsyncTask *rinzu_aot_dispatcher_ready_task(RinzuAsyncState *state, unsigned lane) {
  RinzuAsyncTask *ready = NULL;
  for (RinzuAsyncTask *task = state->ready_head; task; task = task->ready_next)
    if (rinzu_aot_async_runnable(task) && (!!task->children.closing) == (lane == 0) &&
        (!ready || task->future->identity < ready->future->identity))
      ready = task;
  return ready;
}
static int rinzu_aot_dispatcher_ready(void *raw, unsigned lane) {
  RinzuAsyncState *state = raw;
  rinzu_async_owner(&state->runtime);
  return rinzu_aot_dispatcher_ready_task(state, lane) != NULL ||
         (lane == 1 && (rinzu_async_has_completion(&state->runtime) || state->completion_head));
}
static int rinzu_aot_dispatcher_can_enter(void *raw) {
  RinzuAsyncState *state = raw;
  rinzu_async_owner(&state->runtime);
  rinzu_owner_transition_t *owner = rinzu_context_transitions(state->root);
  return !state->root->callback_depth &&
         (owner->state == RINZU_OWNER_MANAGED || owner->state == RINZU_OWNER_NATIVE_SAFE);
}
static int rinzu_aot_dispatcher_step(void *raw, unsigned lane) {
  RinzuAsyncState *state = raw;
  rinzu_async_owner(&state->runtime);
  RinzuContext *root = state->root;
  rinzu_owner_transition_t *owner = rinzu_context_transitions(root);
  int native = owner->state == RINZU_OWNER_NATIVE_SAFE, did = 0;
  rinzu_owner_callback_scope_t scope = {0};
  if (native)
    scope = rinzu_owner_callback_enter(owner);
  RinzuAsyncTask *task = rinzu_aot_dispatcher_ready_task(state, lane);
  if (task) {
    rinzu_aot_async_resume(root, task);
    did = 1;
  } else if (lane == 1)
    did = rinzu_aot_async_collect_ready(state);
  if (native)
    rinzu_owner_callback_leave(scope);
  return did;
}
static void rinzu_aot_completion_wake(void *raw) {
  rinzu_foreign_wake(raw);
}
#endif
static void rinzu_aot_control_interrupt(RinzuContext *context, rinzu_async_future_t *future) {
  RinzuAsyncState *state = rinzu_aot_async_state(context);
  if (future->settled ||
      atomic_load_explicit(&future->terminal, memory_order_acquire) != RINZU_ASYNC_PENDING)
    return;
  RinzuAsyncTask *task = rinzu_aot_async_task(state, future);
  if (task) {
    task->context.shutdown_requested = 1;
    if (!task->context.cancel_masked && (task->waiting || task->waiting_for_completion))
      rinzu_aot_async_resume(context, task);
  } else {
#ifdef RINZU_AOT_HAS_SUPPLANT
    if (rinzu_aot_supplant_cancel(context, future))
      return;
#endif
    (void)rinzu_async_cancel(future);
  }
}
static int rinzu_aot_async_root_shutdown(RinzuContext *context) {
  return context->async && context->async->explicit_shutdown;
}
int64_t rinzu_baseline_task_control(void *raw, int64_t kind, int64_t handle) {
  RinzuContext *context = raw;
  RinzuAsyncState *state = rinzu_aot_async_state(context);
  if (kind == 4) {
    if (handle < 0 || handle > 255)
      return 0;
    state->exit_status = (int)handle;
    return 1;
  }
  if (kind == 3) {
    int first = !state->explicit_shutdown;
    state->explicit_shutdown = 1;
    state->root->shutdown_requested = 1;
    rinzu_owner_request_shutdown(rinzu_context_transitions(state->root));
    return first;
  }
  if (kind < 0 || kind > 2)
    rinzu_failure_fatal();
  rinzu_async_future_t *future = rinzu_async_find(&state->runtime, (uint64_t)handle);
  int ready = future->settled || atomic_load_explicit(&future->physical_done, memory_order_acquire);
  if (kind == 0)
    return ready;
  if (ready || atomic_load_explicit(&future->terminal, memory_order_acquire) != RINZU_ASYNC_PENDING)
    return 0;
  int won = kind == 2 ? rinzu_async_timeout(future) : rinzu_async_cancel(future);
  if (!won)
    return 0;
  if (future->scope && rinzu_child_expect_control(future->scope, future->identity))
    rinzu_failure_fatal();
  RinzuAsyncTask *task = rinzu_aot_async_task(state, future);
  if (task) {
    task->context.cancel_requested = 1;
    task->context.cancel_masked = 0;
    if (task->waiting || task->waiting_for_completion)
      rinzu_aot_async_resume(context, task);
  }
#ifdef RINZU_AOT_HAS_SUPPLANT
  else
    (void)rinzu_aot_supplant_cancel(context, future);
#endif
  return 1;
}
static void rinzu_aot_async_pump(RinzuContext *context, rinzu_async_future_t *wanted) {
  RinzuAsyncState *state = rinzu_aot_async_state(context);
  while (!wanted->settled) {
    if (state->explicit_shutdown && !context->pending.depth && !context->cancel_masked)
      rinzu_aot_control_interrupt(context, wanted);
    uint64_t observed = rinzu_async_completion_epoch(&state->runtime);
#ifdef RINZU_AOT_HAS_SUPPLANT
    if (rinzu_aot_supplant_drive(context))
      continue;
#endif
    rinzu_aot_async_resume_ready(context);
    if (wanted->settled)
      break;
#ifdef RINZU_AOT_HAS_SUPPLANT
    /* A queued managed body can reserve a supplant during this ready turn.
       Drive that reservation before following its internal TASK future: it
       belongs to the native gate, not to a managed continuation. */
    if (rinzu_aot_supplant_drive(context))
      continue;
#endif
    rinzu_async_future_t *future = wanted;
    uint64_t depth = 0;
    while (future->operation == RINZU_ASYNC_TASK && !future->settled) {
      RinzuAsyncTask *task = rinzu_aot_async_task(state, future);
      if (!task || ++depth > state->runtime.next_identity)
        rinzu_failure_fatal();
      if (task->waiting_for_completion) {
        future = NULL;
        break;
      }
      if (!task->waiting)
        rinzu_failure_fatal();
      future = task->waiting;
    }
    if (future) {
      if (future->settled || future->operation == RINZU_ASYNC_TASK)
        rinzu_failure_fatal();
      rinzu_async_start(future);
      if (future->operation == RINZU_ASYNC_READY) {
        rinzu_async_materialize(future);
        rinzu_async_adopt_observed(future, &state->root->pending);
        continue;
      }
    }
    if (!rinzu_aot_async_collect_ready(state))
      rinzu_async_wait_progress(&state->runtime, observed);
  }
}
static void rinzu_aot_async_suspend_for_completion(RinzuAsyncTask *task) {
  RinzuCompletionTurn *turn = &task->completion_turn;
  if (task->waiting || task->waiting_for_completion || task->wait_dependency ||
      turn->task != task || turn->settled)
    rinzu_failure_fatal();
  task->waiting_for_completion = 1;
  task->wait_dependency = rinzu_owner_wait_prepare(
      rinzu_context_transitions(&task->context), task->future->identity + 1, turn->identity + 1,
      task->function_label, task->function_source.file, task->function_source.start_byte,
      task->function_source.end_byte);
  rinzu_continuation_yield(&task->continuation);
  task->waiting_for_completion = 0;
  if (turn->settled)
    rinzu_aot_async_finish_completion_turn(turn);
}
static void rinzu_aot_async_wait_completion_turn(RinzuContext *context, uint64_t identity) {
  RinzuAsyncState *state = context->async;
  RinzuAsyncTask *task = context->task;
  if (!task || task->completion_turn.task || !state->runtime.scope ||
      (state->completion_tail && state->completion_tail->identity >= identity))
    rinzu_failure_fatal();
  /* This record lives with the task. If control interrupts the await, its
     scope still drains the scheduled completion after running finally. */
  RinzuCompletionTurn *turn = &task->completion_turn;
  *turn = (RinzuCompletionTurn){.identity = identity,
                                .task = task,
                                .scope = state->runtime.scope,
                                .previous = state->completion_tail};
  if (turn->previous)
    turn->previous->next = turn;
  else
    state->completion_head = turn;
  state->completion_tail = turn;
  turn->ready = rinzu_owner_ready_prepare(rinzu_context_transitions(context),
                                          "reactor.native-completion", context->source.file,
                                          context->source.start_byte, context->source.end_byte);
  rinzu_ready_publish(turn->ready);
  do {
    rinzu_aot_async_suspend_for_completion(task);
    if ((context->cancel_requested || context->shutdown_requested) && !context->pending.depth &&
        !context->cancel_masked) {
      rinzu_failure_fail(&context->pending, context->shutdown_requested ? 4 : 2, context->source);
      return;
    }
  } while (turn->task);
}
static void rinzu_aot_async_wait(RinzuContext *context, rinzu_async_future_t *future) {
  if (context->task) {
    context->task->waiting = future;
    RinzuAsyncTask *task = context->task;
    task->wait_dependency = rinzu_owner_wait_prepare(
        rinzu_context_transitions(context), task->future->identity + 1, future->identity + 1,
        task->function_label, task->function_source.file, task->function_source.start_byte,
        task->function_source.end_byte);
    task->readiness.on_settled = rinzu_aot_async_waiter_ready;
    rinzu_async_waiter_register(
        future, &task->readiness,
        rinzu_owner_ready_prepare(rinzu_context_transitions(context), task->function_label,
                                  task->function_source.file, task->function_source.start_byte,
                                  task->function_source.end_byte));
    rinzu_continuation_yield(&context->task->continuation);
    context->task->waiting = NULL;
  } else {
    RinzuAsyncState *state = rinzu_aot_async_state(context);
    rinzu_wait_dependency_t *wait_dependency = rinzu_owner_wait_prepare(
        rinzu_context_transitions(context), 0, future->identity + 1, state->root_label,
        state->root_source.file, state->root_source.start_byte, state->root_source.end_byte);
    rinzu_async_ready_waiter_t readiness = {0};
    rinzu_async_waiter_register(
        future, &readiness,
        rinzu_owner_ready_prepare(rinzu_context_transitions(context), state->root_label,
                                  state->root_source.file, state->root_source.start_byte,
                                  state->root_source.end_byte));
    int resume_managed = state->root_activity != NULL;
    if (state->root_activity) {
      rinzu_activity_end(state->root_activity);
      state->root_activity = NULL;
    }
    rinzu_aot_async_pump(context, future);
    rinzu_activity_wait_end(wait_dependency);
    rinzu_async_waiter_receive(&readiness);
    rinzu_owner_note_turn(rinzu_context_transitions(context));
    /* Scope closing also pumps children after the root body has returned.
       Only a suspended body resumes a managed activation here. */
    if (resume_managed) {
      state->root_activity = rinzu_owner_activity_prepare(
          rinzu_context_transitions(context), RINZU_ACTIVITY_MANAGED, state->root_label,
          state->root_source.file, state->root_source.start_byte, state->root_source.end_byte,
          context->roots.depth);
      rinzu_activity_start(state->root_activity);
    }
  }
}
int64_t rinzu_baseline_async_await(void *raw, int64_t handle, int64_t consume) {
  RinzuContext *context = raw;
  RinzuAsyncState *state = rinzu_aot_async_state(context);
  if (!state->first_await_seen) {
    state->first_await_seen = 1;
    state->first_await_workers = state->started_workers;
    state->first_await_files = state->started_files;
  }
  if (handle < 0) {
    uint64_t identity = (uint64_t)(-(handle + 1));
    if (!consume || identity >= state->runtime.next_identity)
      rinzu_failure_fatal();
    rinzu_aot_async_wait_completion_turn(context, identity);
    return 0; /* Unit */
  }
  rinzu_async_future_t *future = rinzu_async_find(&state->runtime, (uint64_t)handle);
  int pending = !future->settled;
  if (pending)
    ASYNC_EVENT(raw,
                future->operation == RINZU_ASYNC_WORKER
                    ? RINZU_EVENT_AWAIT_WORKER
                    : (future->operation == RINZU_ASYNC_TASK
                           ? RINZU_EVENT_AWAIT_ASYNC
                           : (future->operation == RINZU_ASYNC_TIMER ? RINZU_EVENT_AWAIT_TIMER
                                                                     : RINZU_EVENT_AWAIT_FILE)),
                future->identity);
  while (!future->settled) {
    rinzu_aot_async_wait(context, future);
    if ((context->cancel_requested || context->shutdown_requested) && !context->pending.depth &&
        !context->cancel_masked) {
      rinzu_failure_fail(&context->pending, context->shutdown_requested ? 4 : 2, context->source);
      return 0;
    }
  }
  uint64_t slot = future->result_slot;
  if (future->scope && rinzu_child_observe(future->scope, future->identity))
    rinzu_failure_fatal();
  if (future->failure.primary) {
    if (context->pending.current.primary)
      rinzu_failure_fatal();
    context->pending.current = rinzu_failure_copy(&future->failure);
  } else {
    if (state->runtime.record_evidence && future->result_kind <= RINZU_SCALAR_BOOL) {
      RinzuAwaitedValue *value = rinzu_failure_allocate(sizeof(*value));
      value->kind = future->result_kind;
      value->bits = slot;
      value->scope = context->task ? context->task->future->identity + 1 : 0;
      value->future = future->identity;
      if (state->values_tail)
        state->values_tail->next = value;
      else
        state->values = value;
      state->values_tail = value;
    }
  }
  ASYNC_EVENT(raw, RINZU_EVENT_AWAIT_READY, future->identity);
  /* Ledger ownership ends at observation. Future/result ownership follows
     the independent consume/GC path below; this block cannot yield or collect. */
  if (future->scope) {
    if (rinzu_child_retire_observed(future->scope, future->identity))
      rinzu_failure_fatal();
    future->scope = NULL;
  }
  if (consume) {
    for (RinzuAsyncResultRoot *root = state->result_roots; root; root = root->next)
      if (root->future_handle == rinzu_async_handle(future)) {
        rinzu_aot_async_release_result(state, root);
        break;
      }
    /* Full proof keeps immutable control evidence, never the payload root. */
    if (!state->runtime.record_evidence)
      rinzu_async_release(future);
  }
  return (int64_t)slot;
}
static void rinzu_aot_async_close_scope(RinzuContext *context, rinzu_child_scope_t *scope,
                                        int allow_cancel) {
  RinzuAsyncState *state = rinzu_aot_async_state(context);
  if (rinzu_child_begin_close(scope, &context->pending.current))
    rinzu_failure_fatal();
  for (;;) {
    if (allow_cancel && (context->cancel_requested || context->shutdown_requested) &&
        !scope->outcome.primary) {
      rinzu_source_position_t source =
          context->task ? context->task->function_source : context->source;
      int64_t cause = context->shutdown_requested ? 4 : 2;
      if (rinzu_child_interrupt_close(scope, source, cause))
        rinzu_failure_fatal();
      rinzu_failure_fail(&context->pending, cause, source);
    }
    if (scope->outcome.primary && rinzu_child_cancels_on_failure(scope, &scope->outcome)) {
      for (rinzu_child_t *child = scope->children; child; child = child->next) {
        if (child->completed)
          continue;
        rinzu_async_future_t *future =
            rinzu_async_find(&state->runtime, RINZU_ASYNC_HANDLE_BASE + child->identity + 1);
        RinzuAsyncTask *task = rinzu_aot_async_task(state, future);
        if (task) {
          if (!task->context.cancel_requested && !task->context.shutdown_requested) {
            /* Request control once without removing the child's inherited
               callback/cleanup mask. Its physical completion is still joined. */
            if (scope->outcome.primary == 4)
              task->context.shutdown_requested = 1;
            else
              task->context.cancel_requested = 1;
            rinzu_aot_async_resume(context, task);
          }
        } else {
#ifdef RINZU_AOT_HAS_SUPPLANT
          if (rinzu_aot_supplant_cancel(context, future))
            continue;
#endif
          (void)rinzu_async_cancel(future);
        }
      }
    }
    rinzu_child_t *pending = scope->children;
    while (pending && pending->completed)
      pending = pending->next;
    if (context->task) {
      RinzuCompletionTurn *turn = &context->task->completion_turn;
      if (turn->scope == scope && (!pending || turn->identity < pending->identity)) {
        rinzu_aot_async_suspend_for_completion(context->task);
        continue;
      }
    }
    if (!pending)
      break;
    rinzu_async_future_t *future =
        rinzu_async_find(&state->runtime, RINZU_ASYNC_HANDLE_BASE + pending->identity + 1);
    rinzu_aot_async_wait(context, future);
  }
  rinzu_aot_async_reparent_scope(state, scope, NULL);
  if (rinzu_child_finish_close(scope))
    rinzu_failure_fatal();
  rinzu_failure_release(context->pending.current);
  context->pending.current = rinzu_failure_copy(&scope->outcome);
}
void rinzu_baseline_task_error_origin(void *raw, int64_t future) {
  RinzuContext *context = raw;
  context->failure_origin =
      future >= (int64_t)RINZU_ASYNC_HANDLE_BASE ? (uint64_t)future - RINZU_ASYNC_HANDLE_BASE : 0;
}
void rinzu_baseline_check_local(void *raw, int64_t bound) {
  RinzuContext *context = raw;
  if (!bound)
    rinzu_failure_fail(&context->pending, 14, context->source);
}
void rinzu_baseline_record_task_error(void *raw, int64_t identity) {
  RinzuContext *context = raw;
  RinzuTaskScope *group = context->task_scopes;
  if (!group || group->identity != (uint64_t)identity || group->cleaning ||
      group->depth != context->roots.depth || !group->body.collect_all)
    rinzu_failure_fatal();
  if (rinzu_child_record_error(&group->body,
                               context->failure_origin ? context->failure_origin - 1 : 0,
                               !!context->failure_origin, &context->pending.current))
    rinzu_failure_fatal();
  rinzu_failure_release(context->pending.current);
  context->pending.current = (rinzu_failure_t){0};
  context->failure_origin = 0;
}
int64_t rinzu_baseline_take_task_errors(void *raw, int64_t identity, int64_t type) {
  RinzuContext *context = raw;
  RinzuTaskScope *group = context->task_scopes;
  if (!group || group->identity != (uint64_t)identity || !group->cleaning ||
      group->depth != context->roots.depth || !group->body.finished || type < 0)
    rinzu_failure_fatal();
  rinzu_cleanup_frame_t *frame = context->pending.suspended;
  while (frame && (!frame->scoped || frame->activation != group->depth ||
                   frame->region != group->cleanup_region))
    frame = frame->previous;
  if (!frame)
    rinzu_failure_fatal();
  rinzu_failure_state_t check = {.current = frame->incoming};
  int fatal = check.current.primary && !rinzu_failure_recoverable(&check);
  size_t count = 0;
  if (!fatal)
    for (rinzu_child_error_t *error = group->body.errors; error; error = error->next) {
      if (count == SIZE_MAX)
        rinzu_failure_fatal();
      ++count;
      for (rinzu_diagnostic_node_t *node = error->failure.head; node; node = node->next) {
        if (count == SIZE_MAX)
          rinzu_failure_fatal();
        ++count;
      }
    }
  if (count > SIZE_MAX / sizeof(rinzu_managed_slot_t))
    rinzu_failure_fatal();
  rinzu_managed_slot_t *slots = rinzu_failure_allocate(count * sizeof(*slots));
  size_t index = 0;
  rinzu_managed_data_t *data = rinzu_context_data(context);
  if (!fatal) {
    for (rinzu_child_error_t *error = group->body.errors; error; error = error->next) {
      if (context->pending.observed_count == INT64_MAX)
        rinzu_failure_fatal();
      rinzu_observed_error_t *observed = rinzu_failure_allocate(sizeof(*observed));
      observed->identity = ++context->pending.observed_count;
      observed->failure = rinzu_failure_copy(&error->failure);
      observed->next = context->pending.observed;
      context->pending.observed = observed;
      rinzu_aot_async_observe(context, &error->failure);
      size_t before = data->objects;
      slots[index++] = (rinzu_managed_slot_t){
          rinzu_managed_failure_handle(data, error->failure.primary, error->failure.primary_source,
                                       error->failure.primary_error),
          1};
      for (rinzu_diagnostic_node_t *node = error->failure.head; node; node = node->next)
        slots[index++] = (rinzu_managed_slot_t){
            rinzu_managed_failure_handle(data, node->cause, node->source, node->error), 1};
      rinzu_failure_forget_managed(&observed->failure);
      context->allocations += data->objects - before;
    }
    rinzu_failure_release(frame->incoming);
    frame->incoming = (rinzu_failure_t){0};
  }
  uint64_t result = 0;
  if (rinzu_managed_composite(data, (size_t)type, -1, slots, count, &result))
    rinzu_failure_fatal();
  free(slots);
  context->allocations += 1;
  return (int64_t)result;
}
void rinzu_baseline_task_scope(void *raw, int64_t identity, int64_t phase) {
  RinzuContext *context = raw;
  RinzuAsyncState *state = rinzu_aot_async_state(context);
  if (phase == 0 || phase == 3) {
    RinzuTaskScope *group = rinzu_failure_allocate(sizeof(*group));
    group->identity = (uint64_t)identity;
    group->depth = context->roots.depth;
    group->parent = state->runtime.scope;
    group->allow_cancel = !context->pending.depth && !context->cancel_masked;
    group->previous = context->task_scopes;
    rinzu_child_init(&group->body);
    rinzu_child_init(&group->cleanup);
    rinzu_child_scope_t *owner = context->task ? &context->task->children : &state->children;
    group->body.task_identity = group->cleanup.task_identity = owner->task_identity;
    group->body.admission_counter = group->cleanup.admission_counter = &owner->admissions;
    group->body.collect_all = group->cleanup.collect_all = phase == 3;
    context->task_scopes = group;
    state->runtime.scope = &group->body;
    return;
  }
  RinzuTaskScope *group = context->task_scopes;
  if (!group || group->identity != (uint64_t)identity || group->depth != context->roots.depth ||
      group->cleaning != (phase == 2) || (phase != 1 && phase != 2) ||
      !context->pending.suspended || context->pending.current.primary)
    rinzu_failure_fatal();
  rinzu_cleanup_frame_t *frame = context->pending.suspended;
  context->pending.current = frame->incoming;
  frame->incoming = (rinzu_failure_t){0};
  rinzu_child_scope_t *children = phase == 1 ? &group->body : &group->cleanup;
  rinzu_aot_async_close_scope(context, children, phase == 1 && group->allow_cancel);
  frame->incoming = context->pending.current;
  context->pending.current = (rinzu_failure_t){0};
  if (phase == 2) {
    rinzu_cleanup_frame_t *outer = frame->previous;
    while (outer && (!outer->scoped || outer->activation != group->depth ||
                     outer->region != group->cleanup_region))
      outer = outer->previous;
    if (outer) {
      rinzu_failure_state_t combined = {0};
      combined.current = outer->incoming;
      rinzu_failure_begin(&combined);
      combined.current = frame->incoming;
      frame->incoming = (rinzu_failure_t){0};
      rinzu_failure_end(&combined);
      outer->incoming = combined.current;
    }
  }
  if (phase == 1) {
    group->cleanup_region = frame->region;
    group->cleaning = 1;
    state->runtime.scope = &group->cleanup;
    return;
  }
  state->runtime.scope = group->parent;
  context->task_scopes = group->previous;
  rinzu_child_dispose(&group->body);
  rinzu_child_dispose(&group->cleanup);
  free(group);
}
static void rinzu_aot_async_close(RinzuContext *context) {
  RinzuAsyncState *state = rinzu_aot_async_state(context);
  if (context != state->root || state->runtime.scope != &state->children)
    rinzu_failure_fatal();
  rinzu_aot_async_close_scope(context, &state->children, 1);
  if (state->completion_head || state->completion_tail)
    rinzu_failure_fatal();
  rinzu_continuation_cache_dispose(&state->continuations);
  while (state->result_roots)
    rinzu_aot_async_release_result(state, state->result_roots);
}
static void rinzu_aot_async_print(RinzuAsyncState *state, int baseline) {
  if (getenv("RINZU_CONTINUATION_DIAGNOSTICS"))
    printf("RINZU_CONTINUATION_CACHE=%zu,%zu,%zu\n", state->continuations.allocations,
           state->continuations.reuses, state->continuations.cache_limit);
  printf("RINZU_FUTURE_ADMISSIONS=");
  int admission_first = 1;
  while (state->admissions) {
    RinzuFutureAdmission *entry = state->admissions;
    state->admissions = entry->next;
    printf("%s%llu:%llu:%llu", admission_first ? "" : ",", (unsigned long long)entry->future,
           (unsigned long long)entry->parent_task, (unsigned long long)entry->ordinal);
    admission_first = 0;
    free(entry);
  }
  printf("\n");
  printf("RINZU_AWAITED_VALUES=");
  int first = 1;
  for (RinzuAwaitedValue *value = state->values; value; value = value->next) {
    printf("%s%u:%016llx", first ? "" : ",", value->kind, (unsigned long long)value->bits);
    first = 0;
  }
  printf("\nRINZU_AWAITED_TASK_VALUES=");
  first = 1;
  while (state->values) {
    RinzuAwaitedValue *value = state->values;
    state->values = value->next;
    printf("%s%llu:%llu:%u:%016llx", first ? "" : ",", (unsigned long long)value->scope,
           (unsigned long long)value->future, value->kind, (unsigned long long)value->bits);
    first = 0;
    free(value);
  }
  printf("\nRINZU_TRACE={\"schema\":\"rinzu.event-trace\",\"version\":1,\"producer\":\"rinzu-aot\","
         "\"context\":{\"resume-domain\":\"main\",\"resources\":\"%s\"},\"events\":[",
         baseline ? "baseline" : "leaked");
  unsigned long long sequence = 0;
  while (state->events) {
    RinzuAsyncEvent *event = state->events;
    state->events = event->next;
    printf("%s{\"sequence\":%llu,\"domain\":\"main\",\"task\":\"main", sequence ? "," : "",
           sequence);
    sequence += 1;
    if (event->scope)
      printf(":%llu", (unsigned long long)event->scope);
    printf("\",");
    switch (event->kind) {
    case RINZU_EVENT_TASK_START:
      printf("\"kind\":\"task-start\"");
      break;
    case RINZU_EVENT_TASK_SUCCESS:
    case RINZU_EVENT_TASK_FAILURE:
    case RINZU_EVENT_TASK_CANCELLED:
      printf("\"kind\":\"task-complete\",\"outcome\":\"%s\"",
             event->kind == RINZU_EVENT_TASK_SUCCESS
                 ? "success"
                 : (event->kind == RINZU_EVENT_TASK_CANCELLED ? "cancelled" : "failure"));
      break;
    case RINZU_EVENT_AWAIT_WORKER:
    case RINZU_EVENT_AWAIT_FILE:
    case RINZU_EVENT_AWAIT_TIMER:
    case RINZU_EVENT_AWAIT_ASYNC:
    case RINZU_EVENT_AWAIT_SUPPLANT:
      printf("\"kind\":\"await-pending\",\"operation\":\"%s\"",
             event->kind == RINZU_EVENT_AWAIT_WORKER
                 ? "spawn"
                 : (event->kind == RINZU_EVENT_AWAIT_ASYNC
                        ? "async"
                        : (event->kind == RINZU_EVENT_AWAIT_SUPPLANT
                               ? "supplant"
                               : (event->kind == RINZU_EVENT_AWAIT_TIMER ? "timer"
                                                                         : "file-read-size"))));
      break;
    case RINZU_EVENT_AWAIT_READY:
      printf("\"kind\":\"await-ready\",\"operation\":\"future:%llu\"",
             (unsigned long long)event->future);
      break;
    case RINZU_EVENT_NATIVE_ENTER:
    case RINZU_EVENT_NATIVE_LEAVE:
    case RINZU_EVENT_CALLBACK_ENTER:
    case RINZU_EVENT_CALLBACK_LEAVE:
      printf("\"kind\":\"%s\",\"transition\":\"%s\"",
             event->kind == RINZU_EVENT_NATIVE_ENTER || event->kind == RINZU_EVENT_CALLBACK_ENTER
                 ? "native-enter"
                 : "native-leave",
             event->kind == RINZU_EVENT_CALLBACK_ENTER || event->kind == RINZU_EVENT_CALLBACK_LEAVE
                 ? "callback-managed"
                 : "native-safe");
      break;
    default:
      rinzu_failure_fatal();
    }
    printf("}");
    free(event);
  }
  printf("]}\n");
}
