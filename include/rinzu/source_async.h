#ifndef RINZU_SOURCE_ASYNC_H
#define RINZU_SOURCE_ASYNC_H
#include "rinzu/failure_transport.h"
#include "rinzu/child_scope.h"
#include "rinzu/scalar_transport.h"
#include "rinzu/typed_transport.h"
#include "rinzu/ready_work.h"
#include "rinzu/resource_counters.h"
#include "rinzu/native_completion.h"
#include "rinzu/thread_identity.h"
#include <stdatomic.h>
#include <errno.h>
#include <string.h>
#include <stdlib.h>
#include <time.h>

/* Private source-continuation records. These identities are disjoint from
   managed data handles and never contain the raw bits of a scalar result. */
#define RINZU_ASYNC_HANDLE_BASE (UINT64_C(1) << 61)
enum {
  RINZU_ASYNC_READY,
  RINZU_ASYNC_WORKER,
  RINZU_ASYNC_FILE,
  RINZU_ASYNC_TASK,
  RINZU_ASYNC_TIMER,
  RINZU_ASYNC_COMPLETION
};
/* Main-only result slots. These kinds never enter the scalar Worker codec. */
enum { RINZU_ASYNC_MANAGED = 12, RINZU_ASYNC_FUTURE = 13 };
enum {
  RINZU_ASYNC_PENDING,
  RINZU_ASYNC_COMPLETED,
  RINZU_ASYNC_CANCELLED,
  RINZU_ASYNC_TIMED_OUT,
  RINZU_ASYNC_SETTLED
};
typedef struct rinzu_async_future rinzu_async_future_t;
typedef struct rinzu_async_native_wake rinzu_async_native_wake_t;
typedef struct rinzu_async_ready_waiter {
  rinzu_async_future_t *future;
  rinzu_ready_work_t *readiness;
  struct rinzu_async_ready_waiter *next;
  /* Owner-only, after settled/result publication. Must not reenter, receive or
     free waiters. Physical completion publishers never invoke this hook. */
  void (*on_settled)(struct rinzu_async_ready_waiter *);
} rinzu_async_ready_waiter_t;
/* Conformance-only intervals. Reserve and release use the resource gate's
   actual order; enabling evidence does not change admission or scheduling. */
typedef struct rinzu_async_credit {
  uint64_t operation, reserved_at, released_at;
  unsigned direction;
  size_t bytes;
  struct rinzu_async_credit *next;
} rinzu_async_credit_t;
typedef struct rinzu_async_diagnostic_packet {
  unsigned char *bytes;
  size_t size;
  rinzu_async_credit_t *credit;
  struct rinzu_async_diagnostic_packet *next;
} rinzu_async_diagnostic_packet_t;
enum {
  RINZU_ASYNC_CONTROL_QUANTUM = 64,
  RINZU_ASYNC_CONTROL_CLASSES = 32,
  RINZU_ASYNC_CONTROL_CACHE_BYTES = 64 * 1024
};
typedef struct rinzu_async_runtime {
  pthread_t owner;
  pthread_mutex_t gate;
  pthread_cond_t completion_changed;
  uint64_t completion_epoch;
  uint64_t next_identity;
  rinzu_async_future_t *futures;
  rinzu_async_future_t **future_index;
  size_t future_capacity;
  rinzu_child_scope_t *scope;
  void (*admission_hook)(rinzu_async_future_t *, void *);
  void *admission_context;
  size_t records, domains, peak_domains, in_flight, peak_in_flight;
  size_t live_main_tasks, main_task_limit, pending_operations, pending_operation_limit;
  size_t transport_bytes, peak_transport_bytes;
  /* Private target profile; production initialization uses the fixed caps. */
  /* Runtime target profile; production defaults below are not SDK settings. */
  size_t process_transport_limit, operation_transport_limit;
  int record_evidence, capture_error_trace;
  unsigned callback_admission;
  uint64_t credit_sequence;
  rinzu_async_credit_t *credits, *credits_tail;
  void (*completion_wake)(void *);
  void *wake_context;
  pthread_t completion_reactor;
  int reactor_started, reactor_stopping, direct_native_completion, active_native_only;
  int native_notifications;
  rinzu_async_native_wake_t *native_wake;
  rinzu_async_future_t *native_completions;
  /* Owner-only dead Task/Future blocks, separate from managed value/root caches. */
  void *control_cache[RINZU_ASYNC_CONTROL_CLASSES];
  size_t control_cached_bytes, control_live;
} rinzu_async_runtime_t;
struct rinzu_async_future {
  uint64_t gc_result;
  unsigned gc_reached;
  rinzu_async_runtime_t *runtime;
  rinzu_child_scope_t *scope;
  rinzu_async_future_t *next, *previous, *index_next;
  /* Owner-only weak link. Cleared before the continuation is freed. */
  void *task_context;
  uint64_t identity;
  uint64_t parent_task, ordinal;
  unsigned operation, result_kind;
  const unsigned *parameter_kinds;
  const rinzu_value_schema_t *value_schema;
  const size_t *parameter_types;
  size_t result_type;
  /* Owner callbacks run only before submission or after physical join. The
     Worker reads immutable schema metadata and its own input/result packets. */
  void *transport_owner;
  void (*encode_input)(rinzu_async_future_t *);
  uint64_t (*materialize_result)(rinzu_async_future_t *);
  size_t parameter_count;
  uint64_t *arguments;
  unsigned char *input, *result, *native_payload;
  size_t input_size, result_size;
  rinzu_async_credit_t *input_credit, *result_credit;
  uint64_t result_slot;
  rinzu_failure_t failure;
  rinzu_observed_error_t *observed;
  rinzu_async_diagnostic_packet_t *observed_packets;
  size_t observed_size;
  rinzu_source_position_t source;
  void (*run)(rinzu_async_future_t *);
  int admitted, executed, settled, result_is_failure;
  int accounting_pending, accounting_task, main_admitted;
  int result_rejected, heap_evidence, worker_body_entered;
  size_t worker_objects, worker_roots, worker_peak_objects, worker_peak_frames;
  uint64_t worker_collections;
  size_t worker_userdata_scopes, worker_userdata_tokens, worker_userdata_roots,
      worker_userdata_in_flight;
  size_t worker_userdata_created, worker_userdata_released;
  pthread_t thread;
  int started, joined;
  rinzu_ready_work_t *readiness;
  /* runtime->gate serializes native-only request delivery with Worker detach.
     This target never grants Origin access to the Worker heap or frames. */
  rinzu_activity_board_t *native_board;
  /* Owner-only dependency links; publishers/watchdog never inspect them. */
  rinzu_async_ready_waiter_t *ready_waiters;
  _Atomic unsigned terminal;
  _Atomic int cancel_requested, physical_done;
  rinzu_native_completion_t *native_completion;
  int native_notified;
  rinzu_async_future_t *completion_next;
};
static inline void rinzu_async_owner(rinzu_async_runtime_t *runtime) {
  if (!rinzu_thread_equal(runtime->owner, pthread_self()))
    rinzu_failure_fatal();
}
/* Private calls below follow the existing public admission/release Owner checks.
   All identity, roots, publisher and cleanup obligations precede release. */
static inline void *rinzu_async_control_allocate(rinzu_async_runtime_t *runtime, size_t size) {
  if (size < sizeof(void *) || size > RINZU_ASYNC_CONTROL_QUANTUM * RINZU_ASYNC_CONTROL_CLASSES)
    return rinzu_failure_allocate(size);
  size_t index = (size - 1) / RINZU_ASYNC_CONTROL_QUANTUM;
  size_t bytes = (index + 1) * RINZU_ASYNC_CONTROL_QUANTUM;
  if (runtime->control_live == SIZE_MAX ||
      runtime->control_cached_bytes > RINZU_ASYNC_CONTROL_CACHE_BYTES)
    rinzu_failure_fatal();
  void *memory = runtime->control_cache[index];
  if (memory) {
    if (runtime->control_cached_bytes < bytes)
      rinzu_failure_fatal();
    runtime->control_cache[index] = *(void **)memory;
    runtime->control_cached_bytes -= bytes;
    memset(memory, 0, bytes);
  } else {
    memory = rinzu_failure_allocate(bytes);
    rinzu_resource_acquire(RINZU_RESOURCE_RUNTIME_CONTROL, bytes);
  }
  ++runtime->control_live;
  return memory;
}
static inline void rinzu_async_control_release(rinzu_async_runtime_t *runtime, void *memory,
                                               size_t size) {
  if (size < sizeof(void *) || size > RINZU_ASYNC_CONTROL_QUANTUM * RINZU_ASYNC_CONTROL_CLASSES) {
    free(memory);
    return;
  }
  size_t index = (size - 1) / RINZU_ASYNC_CONTROL_QUANTUM;
  size_t bytes = (index + 1) * RINZU_ASYNC_CONTROL_QUANTUM;
  if (!memory || !runtime->control_live ||
      runtime->control_cached_bytes > RINZU_ASYNC_CONTROL_CACHE_BYTES)
    rinzu_failure_fatal();
  --runtime->control_live;
  if (bytes <= RINZU_ASYNC_CONTROL_CACHE_BYTES - runtime->control_cached_bytes) {
    /* Cached allocations hold a next pointer in their first word. */
    *(void **)memory = runtime->control_cache[index];
    runtime->control_cache[index] = memory;
    runtime->control_cached_bytes += bytes;
  } else {
    rinzu_resource_release(RINZU_RESOURCE_RUNTIME_CONTROL, bytes);
    free(memory);
  }
}
static inline void rinzu_async_control_dispose(rinzu_async_runtime_t *runtime) {
  if (runtime->control_live)
    rinzu_failure_fatal();
  for (size_t index = 0; index < RINZU_ASYNC_CONTROL_CLASSES; ++index) {
    size_t bytes = (index + 1) * RINZU_ASYNC_CONTROL_QUANTUM;
    while (runtime->control_cache[index]) {
      void *memory = runtime->control_cache[index];
      if (runtime->control_cached_bytes < bytes)
        rinzu_failure_fatal();
      runtime->control_cache[index] = *(void **)memory;
      runtime->control_cached_bytes -= bytes;
      rinzu_resource_release(RINZU_RESOURCE_RUNTIME_CONTROL, bytes);
      free(memory);
    }
  }
  if (runtime->control_cached_bytes)
    rinzu_failure_fatal();
}
static inline void rinzu_async_init(rinzu_async_runtime_t *runtime) {
  memset(runtime, 0, sizeof(*runtime));
  runtime->owner = pthread_self();
  runtime->capture_error_trace = rinzu_error_trace_enabled();
  const char *direct_native = getenv("RINZU_NATIVE_COMPLETION_DIRECT");
  runtime->direct_native_completion = !direct_native || strcmp(direct_native, "0") != 0;
  const char *active_native = getenv("RINZU_NATIVE_COMPLETION_ACTIVE_ONLY");
  runtime->active_native_only = !active_native || strcmp(active_native, "0") != 0;
  const char *notifications = getenv("RINZU_NATIVE_COMPLETION_NOTIFY");
  runtime->native_notifications = !notifications || strcmp(notifications, "0") != 0;
  runtime->main_task_limit = SIZE_MAX;
  runtime->pending_operation_limit = 8192;
  runtime->process_transport_limit = 256 * 1024 * 1024;
  runtime->operation_transport_limit = 128 * 1024 * 1024;
  if (pthread_mutex_init(&runtime->gate, NULL) ||
      pthread_cond_init(&runtime->completion_changed, NULL))
    rinzu_failure_fatal();
}
static inline void rinzu_async_lock(rinzu_async_runtime_t *runtime) {
  if (pthread_mutex_lock(&runtime->gate))
    rinzu_failure_fatal();
}
static inline void rinzu_async_unlock(rinzu_async_runtime_t *runtime) {
  if (pthread_mutex_unlock(&runtime->gate))
    rinzu_failure_fatal();
}
/* The runtime gate serializes Owner and reactor publication. Only SDK done
   authorizes physical completion; a terminal cancellation does not. No SDK
   pointer is read again once physical_done has been published. */
static inline int rinzu_async_publish_native_locked(rinzu_async_future_t *future) {
  if (atomic_load_explicit(&future->physical_done, memory_order_acquire) ||
      !rinzu_native_completion_is_done(future->native_completion))
    return 0;
  if (future->result_kind == RINZU_SCALAR_I64)
    future->result_slot =
        (uint64_t)rinzu_native_completion_transfer_result(future->native_completion);
  unsigned pending = RINZU_ASYNC_PENDING;
  (void)atomic_compare_exchange_strong_explicit(&future->terminal, &pending, RINZU_ASYNC_COMPLETED,
                                                memory_order_acq_rel, memory_order_acquire);
  if (future->readiness)
    rinzu_ready_publish(future->readiness);
  atomic_store_explicit(&future->physical_done, 1, memory_order_release);
  if (future->runtime->completion_epoch == UINT64_MAX)
    rinzu_failure_fatal();
  future->runtime->completion_epoch++;
  return 1;
}
static inline int rinzu_async_scan_native_locked(rinzu_async_runtime_t *runtime) {
  int published = 0;
  rinzu_async_future_t **cursor = &runtime->native_completions;
  while (*cursor) {
    rinzu_async_future_t *future = *cursor;
    published |= rinzu_async_publish_native_locked(future);
    if (runtime->active_native_only &&
        atomic_load_explicit(&future->physical_done, memory_order_acquire)) {
      *cursor = future->completion_next;
      /* A detached record remains a usable Future. The self-link is private
         registry state, not a GC root and not an SDK carrier reference. */
      future->completion_next = future;
    } else {
      cursor = &future->completion_next;
    }
  }
  if (published)
    pthread_cond_broadcast(&runtime->completion_changed);
  return published;
}
/* A producer owns one reference per registered carrier, independently of its
   Future and of the runtime. The target gate pins runtime/wake_context during a
   callback. Teardown unbinds outside runtime->gate, so lock order is always
   target gate -> runtime gate. A delayed post-exchange callback then no-ops.
   Its code belongs to the existing process-pinned compiled runtime image,
   never to the shorter-lived managed/JIT function module. */
struct rinzu_async_native_wake {
  rinzu_native_completion_wake_t header;
  _Atomic size_t references;
  pthread_mutex_t gate;
  rinzu_async_runtime_t *runtime;
};
static void rinzu_async_native_wake_release(rinzu_native_completion_wake_t *raw) {
  rinzu_async_native_wake_t *target = (rinzu_async_native_wake_t *)raw;
  size_t before = atomic_fetch_sub_explicit(&target->references, 1, memory_order_acq_rel);
  if (!before)
    rinzu_failure_fatal();
  if (before == 1) {
    if (pthread_mutex_destroy(&target->gate))
      rinzu_failure_fatal();
    rinzu_resource_release(RINZU_RESOURCE_RUNTIME_CONTROL, sizeof(*target));
    free(target);
  }
}
static void rinzu_async_native_wake_notify(rinzu_native_completion_wake_t *raw) {
  rinzu_async_native_wake_t *target = (rinzu_async_native_wake_t *)raw;
  if (pthread_mutex_lock(&target->gate))
    rinzu_failure_fatal();
  rinzu_async_runtime_t *runtime = target->runtime;
  if (runtime) {
    /* Nudge the Owner, which batches carrier scans and typed materialization.
       Never scan the heap or all waiters on the SDK producer thread. */
    rinzu_async_lock(runtime);
    if (runtime->completion_epoch == UINT64_MAX)
      rinzu_failure_fatal();
    runtime->completion_epoch++;
    pthread_cond_broadcast(&runtime->completion_changed);
    rinzu_async_unlock(runtime);
    if (runtime->completion_wake)
      runtime->completion_wake(runtime->wake_context);
  }
  if (pthread_mutex_unlock(&target->gate))
    rinzu_failure_fatal();
}
static inline void rinzu_async_register_native_wake(rinzu_async_future_t *future) {
  rinzu_async_runtime_t *runtime = future->runtime;
  rinzu_async_native_wake_t *target = runtime->native_wake;
  if (!target) {
    target = rinzu_failure_allocate(sizeof(*target));
    target->header.notify = rinzu_async_native_wake_notify;
    target->header.release = rinzu_async_native_wake_release;
    atomic_init(&target->references, 1);
    target->runtime = runtime;
    if (pthread_mutex_init(&target->gate, NULL))
      rinzu_failure_fatal();
    rinzu_resource_acquire(RINZU_RESOURCE_RUNTIME_CONTROL, sizeof(*target));
    runtime->native_wake = target;
  }
  size_t references = atomic_fetch_add_explicit(&target->references, 1, memory_order_relaxed);
  if (!references || references == SIZE_MAX)
    rinzu_failure_fatal();
  uintptr_t expected = 0;
  rinzu_native_completion_v2_t *completion =
      (rinzu_native_completion_v2_t *)future->native_completion;
  if (!atomic_compare_exchange_strong_explicit(&completion->notification, &expected,
                                               (uintptr_t)&target->header, memory_order_acq_rel,
                                               memory_order_acquire)) {
    target->header.release(&target->header);
    if (expected != RINZU_NATIVE_COMPLETION_DONE && expected != (uintptr_t)&target->header)
      rinzu_failure_fatal();
  }
}
static inline void rinzu_async_native_progress(rinzu_async_runtime_t *runtime) {
  if ((!runtime->direct_native_completion && !runtime->native_wake) || !runtime->native_completions)
    return;
  rinzu_async_lock(runtime);
  (void)rinzu_async_scan_native_locked(runtime);
  rinzu_async_unlock(runtime);
  /* The Owner is executing already; do not introduce reentrant wake hooks. */
}
static inline uint64_t rinzu_async_completion_epoch(rinzu_async_runtime_t *runtime) {
  rinzu_async_owner(runtime);
  rinzu_async_lock(runtime);
  if (runtime->direct_native_completion || runtime->native_wake)
    (void)rinzu_async_scan_native_locked(runtime);
  uint64_t epoch = runtime->completion_epoch;
  rinzu_async_unlock(runtime);
  return epoch;
}
static inline void rinzu_async_wait_progress(rinzu_async_runtime_t *runtime, uint64_t observed) {
  rinzu_async_owner(runtime);
  rinzu_async_lock(runtime);
  if (runtime->direct_native_completion || runtime->native_wake)
    (void)rinzu_async_scan_native_locked(runtime);
  while (runtime->completion_epoch == observed)
    if (pthread_cond_wait(&runtime->completion_changed, &runtime->gate))
      rinzu_failure_fatal();
  rinzu_async_unlock(runtime);
}
static inline void rinzu_async_attach_native_board(rinzu_async_future_t *future,
                                                   rinzu_activity_board_t *board) {
  if (!board || !rinzu_thread_equal(board->owner, pthread_self()))
    _Exit(86);
  rinzu_async_lock(future->runtime);
  if (future->native_board)
    _Exit(86);
  future->native_board = board;
  if (atomic_load_explicit(&future->cancel_requested, memory_order_acquire))
    rinzu_activity_request_shutdown(board);
  rinzu_async_unlock(future->runtime);
}
static inline void rinzu_async_detach_native_board(rinzu_async_future_t *future) {
  rinzu_async_lock(future->runtime);
  if (future->native_board && !rinzu_thread_equal(future->native_board->owner, pthread_self()))
    _Exit(86);
  future->native_board = NULL;
  rinzu_async_unlock(future->runtime);
}
static inline void rinzu_async_request_native_shutdown(rinzu_async_future_t *future) {
  rinzu_async_lock(future->runtime);
  if (future->native_board)
    rinzu_activity_request_shutdown(future->native_board);
  rinzu_async_unlock(future->runtime);
}
/* Configure before admission. The immutable native target remains pinned until
   all real helper threads have joined and the runtime is disposed. */
static inline void rinzu_async_set_completion_wake(rinzu_async_runtime_t *runtime,
                                                   void (*wake)(void *), void *context) {
  rinzu_async_owner(runtime);
  if (runtime->reactor_stopping || runtime->futures || runtime->completion_wake || !wake)
    rinzu_failure_fatal();
  runtime->completion_wake = wake;
  runtime->wake_context = context;
}
static inline void rinzu_async_index_resize(rinzu_async_runtime_t *runtime, size_t capacity) {
  if (capacity < 32 || (capacity & (capacity - 1)) ||
      capacity > SIZE_MAX / sizeof(*runtime->future_index))
    rinzu_failure_fatal();
  rinzu_async_future_t **index = rinzu_failure_allocate(capacity * sizeof(*index));
  for (rinzu_async_future_t *future = runtime->futures; future; future = future->next) {
    size_t bucket = rinzu_identity_bucket(future->identity, capacity);
    future->index_next = index[bucket];
    index[bucket] = future;
  }
  free(runtime->future_index);
  runtime->future_index = index;
  runtime->future_capacity = capacity;
}
/* A GC-dead snapshot handle may no longer be present. Identity never recycles. */
static inline rinzu_async_future_t *rinzu_async_lookup(rinzu_async_runtime_t *runtime,
                                                       uint64_t handle) {
  rinzu_async_owner(runtime);
  if (handle <= RINZU_ASYNC_HANDLE_BASE || !runtime->future_capacity)
    return NULL;
  uint64_t identity = handle - RINZU_ASYNC_HANDLE_BASE - 1;
  size_t bucket = rinzu_identity_bucket(identity, runtime->future_capacity);
  for (rinzu_async_future_t *future = runtime->future_index[bucket]; future;
       future = future->index_next)
    if (future->identity == identity)
      return future;
  return NULL;
}
static inline rinzu_async_future_t *rinzu_async_new(rinzu_async_runtime_t *runtime,
                                                    unsigned operation, unsigned result_kind,
                                                    rinzu_source_position_t source) {
  rinzu_async_owner(runtime);
  if (runtime->next_identity >= RINZU_ASYNC_HANDLE_BASE - 1 ||
      (rinzu_transfer_width(result_kind) == SIZE_MAX &&
       !(operation == RINZU_ASYNC_TASK &&
         (result_kind == RINZU_ASYNC_MANAGED || result_kind == RINZU_ASYNC_FUTURE))))
    rinzu_failure_fatal();
  if (runtime->records >= runtime->future_capacity) {
    if (runtime->future_capacity > SIZE_MAX / 2)
      rinzu_failure_fatal();
    rinzu_async_index_resize(runtime, runtime->future_capacity ? runtime->future_capacity * 2 : 32);
  }
  rinzu_async_future_t *future = rinzu_async_control_allocate(runtime, sizeof(*future));
  atomic_init(&future->terminal, RINZU_ASYNC_PENDING);
  atomic_init(&future->cancel_requested, 0);
  atomic_init(&future->physical_done, 0);
  future->runtime = runtime;
  future->identity = runtime->next_identity++;
  future->scope = runtime->scope;
  if (future->scope) {
    future->parent_task = future->scope->task_identity;
    uint64_t *counter = future->scope->admission_counter ? future->scope->admission_counter
                                                         : &future->scope->admissions;
    if (*counter == UINT64_MAX)
      rinzu_failure_fatal();
    future->ordinal = (*counter)++;
  }
  if (future->scope && rinzu_child_admit_with_lease(future->scope, future->identity,
                                                    runtime->callback_admission != 0))
    rinzu_failure_fatal();
  future->operation = operation;
  future->result_kind = result_kind;
  future->source = source;
  future->next = runtime->futures;
  if (future->next)
    future->next->previous = future;
  runtime->futures = future;
  size_t bucket = rinzu_identity_bucket(future->identity, runtime->future_capacity);
  future->index_next = runtime->future_index[bucket];
  runtime->future_index[bucket] = future;
  runtime->records += 1;
  rinzu_resource_acquire(RINZU_RESOURCE_FUTURE, 1);
  if (operation == RINZU_ASYNC_FILE || operation == RINZU_ASYNC_TIMER ||
      operation == RINZU_ASYNC_COMPLETION) {
    /* Completion observes an SDK operation already started under its own
       admission bound; never reject its physical drain receipt. */
    if (operation != RINZU_ASYNC_COMPLETION &&
        runtime->pending_operations >= runtime->pending_operation_limit) {
      future->failure.primary = 3;
      future->failure.primary_source = source;
    } else {
      ++runtime->pending_operations;
      rinzu_resource_acquire(RINZU_RESOURCE_PENDING_OPERATION, 1);
      future->accounting_pending = 1;
    }
  }
  if (operation == RINZU_ASYNC_TASK || operation == RINZU_ASYNC_WORKER) {
    rinzu_resource_acquire(RINZU_RESOURCE_TASK, 1);
    future->accounting_task = 1;
  }
  if (runtime->admission_hook)
    runtime->admission_hook(future, runtime->admission_context);
  return future;
}
static inline uint64_t rinzu_async_handle(const rinzu_async_future_t *future) {
  return RINZU_ASYNC_HANDLE_BASE + future->identity + 1;
}
/* Prepare on Origin before submission; only a real packet publication starts
   the interval, and physical join pins the publisher through Owner receipt. */
static inline void rinzu_async_set_readiness(rinzu_async_future_t *future,
                                             rinzu_ready_work_t *readiness) {
  rinzu_async_owner(future->runtime);
  if (!readiness || future->readiness || future->executed || future->settled)
    _Exit(86);
  future->readiness = readiness;
}
static inline void rinzu_async_waiter_register(rinzu_async_future_t *future,
                                               rinzu_async_ready_waiter_t *waiter,
                                               rinzu_ready_work_t *readiness) {
  rinzu_async_owner(future->runtime);
  if (!waiter || waiter->future || waiter->readiness || !readiness)
    _Exit(86);
  waiter->future = future;
  waiter->readiness = readiness;
  waiter->next = future->ready_waiters;
  future->ready_waiters = waiter;
  if (future->settled) {
    rinzu_ready_publish(readiness);
    if (waiter->on_settled)
      waiter->on_settled(waiter);
  }
}
static inline void rinzu_async_waiter_receive(rinzu_async_ready_waiter_t *waiter) {
  if (!waiter || !waiter->future || !waiter->readiness)
    _Exit(86);
  rinzu_async_future_t *future = waiter->future;
  rinzu_async_owner(future->runtime);
  rinzu_async_ready_waiter_t **cursor = &future->ready_waiters;
  while (*cursor && *cursor != waiter)
    cursor = &(*cursor)->next;
  if (!*cursor)
    _Exit(86);
  *cursor = waiter->next;
  rinzu_ready_receive(waiter->readiness);
  rinzu_ready_dispose(waiter->readiness);
  *waiter = (rinzu_async_ready_waiter_t){0};
}
static inline rinzu_async_future_t *rinzu_async_find(rinzu_async_runtime_t *runtime,
                                                     uint64_t handle) {
  rinzu_async_future_t *future = rinzu_async_lookup(runtime, handle);
  if (future)
    return future;
  rinzu_failure_fatal();
  return NULL;
}
static inline rinzu_async_future_t *
rinzu_async_worker(rinzu_async_runtime_t *runtime, const unsigned *kinds, const uint64_t *arguments,
                   size_t count, unsigned result_kind, void (*run)(rinzu_async_future_t *),
                   rinzu_source_position_t source) {
  size_t size = 0;
  if (!run || count > SIZE_MAX / sizeof(uint64_t) ||
      rinzu_transfer_record_size(kinds, count, &size) || (count && !arguments))
    rinzu_failure_fatal();
  for (size_t index = 0; index < count; ++index)
    if (!rinzu_transfer_valid(kinds[index], arguments[index]))
      rinzu_failure_fatal();
  rinzu_async_future_t *future = rinzu_async_new(runtime, RINZU_ASYNC_WORKER, result_kind, source);
  future->parameter_kinds = kinds;
  future->parameter_count = count;
  future->input_size = size;
  future->run = run;
  if (count) {
    future->arguments = rinzu_failure_allocate(count * sizeof(uint64_t));
    memcpy(future->arguments, arguments, count * sizeof(uint64_t));
  }
  return future;
}
static inline uint64_t rinzu_async_ready(rinzu_async_runtime_t *runtime, unsigned kind,
                                         uint64_t slot, rinzu_source_position_t source) {
  if (!rinzu_transfer_valid(kind, slot))
    rinzu_failure_fatal();
  rinzu_async_future_t *future = rinzu_async_new(runtime, RINZU_ASYNC_READY, kind, source);
  future->result_slot = slot;
  return rinzu_async_handle(future);
}
static inline int rinzu_async_reserve_bytes(rinzu_async_runtime_t *runtime, size_t size) {
  if (size > runtime->operation_transport_limit ||
      runtime->transport_bytes > runtime->process_transport_limit ||
      size > runtime->process_transport_limit - runtime->transport_bytes)
    return 0;
  runtime->transport_bytes += size;
  rinzu_resource_acquire(RINZU_RESOURCE_TRANSPORT, size);
  if (runtime->transport_bytes > runtime->peak_transport_bytes)
    runtime->peak_transport_bytes = runtime->transport_bytes;
  return 1;
}
static inline void rinzu_async_release_bytes(rinzu_async_runtime_t *runtime, size_t size) {
  if (size > runtime->transport_bytes)
    rinzu_failure_fatal();
  runtime->transport_bytes -= size;
  rinzu_resource_release(RINZU_RESOURCE_TRANSPORT, size);
}
/* The caller holds runtime->gate for both the counter and its evidence. */
static inline int rinzu_async_reserve_credit(rinzu_async_future_t *future, unsigned direction,
                                             size_t size, rinzu_async_credit_t **record) {
  rinzu_async_runtime_t *runtime = future->runtime;
  if (!rinzu_async_reserve_bytes(runtime, size))
    return 0;
  if (runtime->record_evidence) {
    if (*record || runtime->credit_sequence == UINT64_MAX)
      rinzu_failure_fatal();
    rinzu_async_credit_t *credit = rinzu_failure_allocate(sizeof(*credit));
    credit->operation = future->identity;
    credit->direction = direction;
    credit->bytes = size;
    credit->reserved_at = ++runtime->credit_sequence;
    if (runtime->credits_tail)
      runtime->credits_tail->next = credit;
    else
      runtime->credits = credit;
    runtime->credits_tail = credit;
    *record = credit;
  }
  return 1;
}
static inline void rinzu_async_release_credit(rinzu_async_runtime_t *runtime, size_t size,
                                              rinzu_async_credit_t **record) {
  rinzu_async_release_bytes(runtime, size);
  if (*record) {
    if ((*record)->released_at || (*record)->bytes != size ||
        runtime->credit_sequence == UINT64_MAX)
      rinzu_failure_fatal();
    (*record)->released_at = ++runtime->credit_sequence;
    *record = NULL;
  }
}
/* Origin encodes; the Worker releases input credit only after full decode. */
static inline int rinzu_async_admit(rinzu_async_future_t *future) {
  rinzu_async_runtime_t *runtime = future->runtime;
  rinzu_async_owner(runtime);
  rinzu_async_lock(runtime);
  if (future->admitted || runtime->domains >= 8 || runtime->in_flight >= 64 ||
      !rinzu_async_reserve_credit(future, 0, future->input_size, &future->input_credit)) {
    rinzu_async_unlock(runtime);
    return 0;
  }
  future->admitted = 1;
  runtime->domains += 1;
  runtime->in_flight += 1;
  rinzu_resource_acquire(RINZU_RESOURCE_TRANSPORT_IN_FLIGHT, 1);
  if (runtime->domains > runtime->peak_domains)
    runtime->peak_domains = runtime->domains;
  if (runtime->in_flight > runtime->peak_in_flight)
    runtime->peak_in_flight = runtime->in_flight;
  rinzu_async_unlock(runtime);
  if (future->input_size)
    future->input = rinzu_failure_allocate(future->input_size);
  if (future->encode_input)
    future->encode_input(future);
  else if (rinzu_transfer_encode_record(future->parameter_kinds, future->arguments,
                                        future->parameter_count, future->input, future->input_size))
    rinzu_failure_fatal();
  free(future->arguments);
  future->arguments = NULL;
  return 1;
}
/* Result admission rejection is an Origin-visible Overloaded failure. Drop
   partial packets on the Worker and keep the Domain slot until reception. */
static inline void rinzu_async_result_overloaded(rinzu_async_future_t *future) {
  rinzu_async_runtime_t *runtime = future->runtime;
  free(future->result);
  future->result = NULL;
  rinzu_async_lock(runtime);
  rinzu_async_release_credit(runtime, future->result_size, &future->result_credit);
  rinzu_async_unlock(runtime);
  future->result_size = 0;
  while (future->observed_packets) {
    rinzu_async_diagnostic_packet_t *packet = future->observed_packets;
    future->observed_packets = packet->next;
    free(packet->bytes);
    rinzu_async_lock(runtime);
    rinzu_async_release_credit(runtime, packet->size, &packet->credit);
    rinzu_async_unlock(runtime);
    free(packet);
  }
  future->observed_size = 0;
  rinzu_failure_state_t observed = {0};
  observed.observed = future->observed;
  rinzu_failure_dispose(&observed);
  future->observed = NULL;
  rinzu_failure_release(future->failure);
  future->failure = (rinzu_failure_t){0};
  future->failure.primary = 3;
  future->failure.primary_source = future->source;
  future->result_is_failure = 0;
  future->result_slot = 0;
  future->result_rejected = 1;
}
static inline void rinzu_async_settle_input(rinzu_async_future_t *future) {
  free(future->input);
  future->input = NULL;
  rinzu_async_lock(future->runtime);
  rinzu_async_release_credit(future->runtime, future->input_size, &future->input_credit);
  future->input_size = 0;
  rinzu_async_unlock(future->runtime);
}
static inline int rinzu_async_cancel(rinzu_async_future_t *future) {
  rinzu_async_owner(future->runtime);
  unsigned pending = RINZU_ASYNC_PENDING;
  int won =
      atomic_compare_exchange_strong_explicit(&future->terminal, &pending, RINZU_ASYNC_CANCELLED,
                                              memory_order_acq_rel, memory_order_acquire);
  if (won) {
    atomic_store_explicit(&future->cancel_requested, 1, memory_order_release);
    if (future->native_completion)
      atomic_store_explicit(&future->native_completion->cancelled, 1, memory_order_release);
    rinzu_async_request_native_shutdown(future);
  }
  return won;
}
static inline int rinzu_async_timeout(rinzu_async_future_t *future) {
  rinzu_async_owner(future->runtime);
  unsigned pending = RINZU_ASYNC_PENDING;
  int won =
      atomic_compare_exchange_strong_explicit(&future->terminal, &pending, RINZU_ASYNC_TIMED_OUT,
                                              memory_order_acq_rel, memory_order_acquire);
  if (won) {
    atomic_store_explicit(&future->cancel_requested, 1, memory_order_release);
    if (future->native_completion)
      atomic_store_explicit(&future->native_completion->cancelled, 1, memory_order_release);
    rinzu_async_request_native_shutdown(future);
  }
  return won;
}
static inline void rinzu_async_publish_completion(rinzu_async_future_t *future) {
  unsigned pending = RINZU_ASYNC_PENDING;
  (void)atomic_compare_exchange_strong_explicit(&future->terminal, &pending, RINZU_ASYNC_COMPLETED,
                                                memory_order_acq_rel, memory_order_acquire);
  atomic_store_explicit(&future->physical_done, 1, memory_order_release);
  if (future->readiness)
    rinzu_ready_publish(future->readiness);
  rinzu_async_lock(future->runtime);
  if (future->runtime->completion_epoch == UINT64_MAX)
    rinzu_failure_fatal();
  future->runtime->completion_epoch++;
  /* The native completion reactor and the Owner share this condition.
     A single signal may wake only the reactor and strand the Owner even
     though its epoch changed. Every publication wakes both observers. */
  if (pthread_cond_broadcast(&future->runtime->completion_changed))
    rinzu_failure_fatal();
  rinzu_async_unlock(future->runtime);
  if (future->runtime->completion_wake)
    future->runtime->completion_wake(future->runtime->wake_context);
}
static inline void *rinzu_async_thread(void *raw) {
  rinzu_async_future_t *future = raw;
  rinzu_async_runtime_t *runtime = future->runtime;
  if (rinzu_thread_equal(runtime->owner, pthread_self()))
    rinzu_failure_fatal();
  if (future->operation == RINZU_ASYNC_WORKER && !future->encode_input) {
    if (future->parameter_count)
      future->arguments = rinzu_failure_allocate(future->parameter_count * sizeof(uint64_t));
    if (rinzu_transfer_decode_record(future->parameter_kinds, future->parameter_count,
                                     future->input, future->input_size, future->arguments))
      rinzu_failure_fatal();
    rinzu_async_settle_input(future);
  }
  if (future->operation == RINZU_ASYNC_WORKER &&
      atomic_load_explicit(&future->cancel_requested, memory_order_acquire)) {
    future->failure.primary = 2;
    future->failure.primary_source = future->source;
    if (future->encode_input)
      rinzu_async_settle_input(future);
    free(future->arguments);
    future->arguments = NULL;
    rinzu_async_publish_completion(future);
    return NULL;
  }
  /* Submitted file reads/timers have no native abort hook. Collect their real return
     even when cancellation has already won, as on the Rust reactor. */
  future->run(future);
  free(future->arguments);
  future->arguments = NULL;
  if (future->operation == RINZU_ASYNC_WORKER) {
    if (future->result_rejected) {
      rinzu_async_publish_completion(future);
      return NULL;
    }
    future->result_is_failure = !!future->failure.primary;
    int reserved;
    if (future->result_is_failure || !future->encode_input) {
      size_t size = future->result_is_failure ? rinzu_failure_transfer_size(&future->failure)
                                              : rinzu_transfer_width(future->result_kind);
      rinzu_async_lock(runtime);
      reserved = rinzu_async_reserve_credit(future, 1, size, &future->result_credit);
      rinzu_async_unlock(runtime);
      if (!reserved) {
        rinzu_async_result_overloaded(future);
        rinzu_async_publish_completion(future);
        return NULL;
      }
      future->result_size = size;
      if (size)
        future->result = rinzu_failure_allocate(size);
      if (future->result_is_failure) {
        if (rinzu_failure_transfer_encode(&future->failure, future->result, size))
          rinzu_failure_fatal();
        rinzu_failure_release(future->failure);
        future->failure = (rinzu_failure_t){0};
      } else if (rinzu_transfer_encode_scalar(future->result_kind, future->result_slot,
                                              future->result, size))
        rinzu_failure_fatal();
    }
    /* Encode local catch diagnostics in observation order, then release every
       Worker-owned native snapshot before the Origin receives completion. */
    rinzu_observed_error_t *ordered = NULL;
    while (future->observed) {
      rinzu_observed_error_t *error = future->observed;
      future->observed = error->next;
      error->next = ordered;
      ordered = error;
    }
    rinzu_async_diagnostic_packet_t **tail = &future->observed_packets;
    while (ordered) {
      rinzu_observed_error_t *error = ordered;
      ordered = error->next;
      rinzu_async_diagnostic_packet_t *packet = rinzu_failure_allocate(sizeof(*packet));
      packet->size = rinzu_failure_transfer_size(&error->failure);
      rinzu_async_lock(runtime);
      reserved = rinzu_async_reserve_credit(future, 1, packet->size, &packet->credit);
      rinzu_async_unlock(runtime);
      if (!reserved) {
        free(packet);
        error->next = ordered;
        future->observed = error;
        rinzu_async_result_overloaded(future);
        rinzu_async_publish_completion(future);
        return NULL;
      }
      if (future->observed_size > SIZE_MAX - packet->size)
        rinzu_failure_fatal();
      future->observed_size += packet->size;
      packet->bytes = rinzu_failure_allocate(packet->size);
      if (rinzu_failure_transfer_encode(&error->failure, packet->bytes, packet->size))
        rinzu_failure_fatal();
      *tail = packet;
      tail = &packet->next;
      rinzu_failure_release(error->failure);
      free(error);
    }
    /* No managed slot or Worker heap reference is read by the Origin. */
    future->result_slot = 0;
  }
  rinzu_async_publish_completion(future);
  return NULL;
}
/* Starting preserves the caller's synchronous prefix. Only join provides
   permission for the Owner to read the Worker/service's native packet. */
static inline void *rinzu_async_completion_reactor(void *raw) {
  rinzu_async_runtime_t *runtime = raw;
  rinzu_async_lock(runtime);
  while (!runtime->reactor_stopping) {
    int published = rinzu_async_scan_native_locked(runtime);
    if (published) {
      /* No future is accessed beyond this unlock. The runtime and wake target
         stay pinned until this single reactor has joined during dispose. */
      rinzu_async_unlock(runtime);
      if (runtime->completion_wake)
        runtime->completion_wake(runtime->wake_context);
      rinzu_async_lock(runtime);
    }
    /* stop may have been requested while the wake callback held no gate.
       Its broadcast preceded this relock; never enter an indefinite wait. */
    if (runtime->reactor_stopping)
      break;
    int polling = 0;
    for (rinzu_async_future_t *future = runtime->native_completions; future;
         future = future->completion_next)
      if (!future->native_notified &&
          !atomic_load_explicit(&future->physical_done, memory_order_acquire)) {
        polling = 1;
        break;
      }
    if (!polling) {
      if (pthread_cond_wait(&runtime->completion_changed, &runtime->gate))
        rinzu_failure_fatal();
      continue;
    }
    struct timespec deadline;
    if (clock_gettime(CLOCK_REALTIME, &deadline))
      rinzu_failure_fatal();
    deadline.tv_nsec += 1000000;
    if (deadline.tv_nsec >= 1000000000) {
      deadline.tv_sec++;
      deadline.tv_nsec -= 1000000000;
    }
    int status = pthread_cond_timedwait(&runtime->completion_changed, &runtime->gate, &deadline);
    if (status && status != ETIMEDOUT)
      rinzu_failure_fatal();
  }
  rinzu_async_unlock(runtime);
  return NULL;
}
static inline void rinzu_async_start(rinzu_async_future_t *future) {
  rinzu_async_owner(future->runtime);
  if (future->executed || future->settled)
    return;
  if (future->runtime->reactor_stopping)
    rinzu_failure_fatal();
  future->executed = 1;
  if (future->operation == RINZU_ASYNC_COMPLETION) {
    if (!rinzu_native_completion_valid(future->native_completion))
      rinzu_failure_fatal();
    rinzu_async_lock(future->runtime);
    future->completion_next = future->runtime->native_completions;
    future->runtime->native_completions = future;
    future->native_notified =
        future->runtime->native_notifications &&
        (future->native_completion->version == 2 || future->native_completion->version == 3);
    if (future->native_notified)
      rinzu_async_register_native_wake(future);
    if (future->runtime->direct_native_completion || future->native_notified)
      /* A cleanup await can share its carrier with a cancelled earlier await.
         Publish every admitted waiter before the SDK may retire that carrier. */
      (void)rinzu_async_scan_native_locked(future->runtime);
    if (!atomic_load_explicit(&future->physical_done, memory_order_acquire) &&
        !future->native_notified && !future->runtime->reactor_started) {
      future->runtime->reactor_started = 1;
      if (pthread_create(&future->runtime->completion_reactor, NULL, rinzu_async_completion_reactor,
                         future->runtime))
        rinzu_failure_fatal();
    }
    pthread_cond_broadcast(&future->runtime->completion_changed);
    rinzu_async_unlock(future->runtime);
    return;
  }
  if (future->failure.primary ||
      atomic_load_explicit(&future->cancel_requested, memory_order_acquire)) {
    rinzu_async_publish_completion(future);
    return;
  }
  if (future->operation == RINZU_ASYNC_READY || future->operation == RINZU_ASYNC_TASK)
    return;
  if (future->operation == RINZU_ASYNC_WORKER && !future->admitted && !rinzu_async_admit(future)) {
    future->failure.primary = 3;
    future->failure.primary_source = future->source;
    rinzu_async_publish_completion(future);
    return;
  }
  int status = pthread_create(&future->thread, NULL, rinzu_async_thread, future);
  if (status == ENOMEM)
    rinzu_failure_fatal();
  if (status) {
    future->failure.primary = 3;
    future->failure.primary_source = future->source;
    rinzu_async_publish_completion(future);
    return;
  }
  future->started = 1;
}
static inline void rinzu_async_join(rinzu_async_future_t *future) {
  rinzu_async_owner(future->runtime);
  if (future->started && !future->joined) {
    if (pthread_join(future->thread, NULL))
      rinzu_failure_fatal();
    future->joined = 1;
    if (!atomic_load_explicit(&future->physical_done, memory_order_acquire))
      rinzu_failure_fatal();
  }
}
static inline void rinzu_async_execute(rinzu_async_future_t *future) {
  rinzu_async_start(future);
  rinzu_async_join(future);
}
/* The receiving Origin decodes before releasing result credit or admission. */
static inline uint64_t rinzu_async_materialize(rinzu_async_future_t *future) {
  rinzu_async_runtime_t *runtime = future->runtime;
  rinzu_async_owner(runtime);
  if (future->settled)
    return future->result_slot;
  if (future->started && !future->joined)
    rinzu_failure_fatal();
  if (future->readiness) {
    rinzu_ready_receive(future->readiness);
    rinzu_activity_owner_turn(future->readiness->board);
  }
  unsigned terminal = atomic_load_explicit(&future->terminal, memory_order_acquire);
  int cancelled = terminal == RINZU_ASYNC_CANCELLED || terminal == RINZU_ASYNC_TIMED_OUT;
  if (cancelled && !future->started && !future->failure.primary) {
    future->failure.primary = 2;
    future->failure.primary_source = future->source;
  }
  uint64_t slot = future->result_slot;
  if (future->result_is_failure) {
    if (rinzu_failure_transfer_decode(future->result, future->result_size, &future->failure))
      rinzu_failure_fatal();
  } else if (future->operation == RINZU_ASYNC_WORKER && !future->failure.primary) {
    if (future->materialize_result)
      slot = future->materialize_result(future);
    else {
      rinzu_scalar_result_t decoded =
          rinzu_transfer_decode_scalar(future->result_kind, future->result, future->result_size);
      if (decoded.status)
        rinzu_failure_fatal();
      slot = decoded.bits;
    }
  }
  while (future->observed_packets) {
    rinzu_async_diagnostic_packet_t *packet = future->observed_packets;
    future->observed_packets = packet->next;
    rinzu_observed_error_t *error = rinzu_failure_allocate(sizeof(*error));
    if (rinzu_failure_transfer_decode(packet->bytes, packet->size, &error->failure))
      rinzu_failure_fatal();
    error->next = future->observed;
    future->observed = error;
    free(packet->bytes);
    rinzu_async_lock(runtime);
    rinzu_async_release_credit(runtime, packet->size, &packet->credit);
    rinzu_async_unlock(runtime);
    free(packet);
  }
  if (cancelled) {
    if (future->failure.primary != 2) {
      rinzu_failure_t late = future->failure;
      future->failure = (rinzu_failure_t){0};
      future->failure.primary = 2;
      future->failure.primary_source = future->source;
      if (late.primary)
        rinzu_child_append_traced_error(&future->failure, late.primary, late.primary_source,
                                        late.primary_error, late.primary_trace);
      rinzu_error_reference_release(late.primary_error);
      rinzu_error_trace_release(late.primary_trace);
      if (late.head) {
        if (future->failure.tail)
          future->failure.tail->next = late.head;
        else
          future->failure.head = late.head;
        future->failure.tail = late.tail;
      }
    }
    slot = 0;
  }
  free(future->input);
  future->input = NULL;
  free(future->result);
  future->result = NULL;
  free(future->arguments);
  future->arguments = NULL;
  rinzu_async_lock(runtime);
  if (future->admitted) {
    rinzu_async_release_credit(runtime, future->input_size, &future->input_credit);
    rinzu_async_release_credit(runtime, future->result_size, &future->result_credit);
    if (!runtime->domains || !runtime->in_flight)
      rinzu_failure_fatal();
    runtime->domains -= 1;
    runtime->in_flight -= 1;
    rinzu_resource_release(RINZU_RESOURCE_TRANSPORT_IN_FLIGHT, 1);
    future->admitted = 0;
  }
  rinzu_async_unlock(runtime);
  future->input_size = 0;
  future->result_size = 0;
  future->observed_size = 0;
  future->result_slot = slot;
  future->settled = 1;
  if (future->accounting_pending) {
    if (!runtime->pending_operations)
      rinzu_failure_fatal();
    --runtime->pending_operations;
    rinzu_resource_release(RINZU_RESOURCE_PENDING_OPERATION, 1);
    future->accounting_pending = 0;
  }
  if (future->accounting_task) {
    rinzu_resource_release(RINZU_RESOURCE_TASK, 1);
    future->accounting_task = 0;
  }
  if (future->main_admitted) {
    if (!runtime->live_main_tasks)
      rinzu_failure_fatal();
    --runtime->live_main_tasks;
    future->main_admitted = 0;
  }
  atomic_store_explicit(&future->terminal, RINZU_ASYNC_SETTLED, memory_order_release);
  if (runtime->capture_error_trace)
    rinzu_failure_mark_unrecorded_traces(&future->failure);
  if (future->scope && rinzu_child_complete(future->scope, future->identity, &future->failure))
    rinzu_failure_fatal();
  for (rinzu_async_ready_waiter_t *waiter = future->ready_waiters; waiter; waiter = waiter->next) {
    rinzu_ready_publish(waiter->readiness);
    if (waiter->on_settled)
      waiter->on_settled(waiter);
  }
  return slot;
}
static inline int rinzu_async_has_completion(rinzu_async_runtime_t *runtime) {
  rinzu_async_owner(runtime);
  rinzu_async_native_progress(runtime);
  for (rinzu_async_future_t *future = runtime->futures; future; future = future->next)
    if (!future->settled &&
        (future->operation == RINZU_ASYNC_WORKER || future->operation == RINZU_ASYNC_FILE ||
         future->operation == RINZU_ASYNC_TIMER || future->operation == RINZU_ASYNC_COMPLETION) &&
        atomic_load_explicit(&future->physical_done, memory_order_acquire))
      return 1;
  return 0;
}
/* A cancellation winner alone does not make a native packet readable. */
static inline int rinzu_async_try_collect(rinzu_async_future_t *future) {
  rinzu_async_owner(future->runtime);
  if (!future->settled && future->executed && future->operation == RINZU_ASYNC_COMPLETION &&
      (future->runtime->direct_native_completion || future->native_notified) &&
      !atomic_load_explicit(&future->physical_done, memory_order_acquire) &&
      rinzu_native_completion_is_done(future->native_completion)) {
    rinzu_async_lock(future->runtime);
    (void)rinzu_async_scan_native_locked(future->runtime);
    rinzu_async_unlock(future->runtime);
  }
  if (future->settled || !future->executed ||
      (future->operation != RINZU_ASYNC_WORKER && future->operation != RINZU_ASYNC_FILE &&
       future->operation != RINZU_ASYNC_TIMER && future->operation != RINZU_ASYNC_COMPLETION) ||
      !atomic_load_explicit(&future->physical_done, memory_order_acquire))
    return 0;
  rinzu_async_join(future);
  rinzu_async_materialize(future);
  return 1;
}
static inline void rinzu_async_adopt_observed(rinzu_async_future_t *future,
                                              rinzu_failure_state_t *state) {
  rinzu_async_owner(future->runtime);
  if (!future->settled)
    rinzu_failure_fatal();
  rinzu_observed_error_t *ordered = NULL;
  while (future->observed) {
    rinzu_observed_error_t *error = future->observed;
    future->observed = error->next;
    error->next = ordered;
    ordered = error;
  }
  while (ordered) {
    rinzu_observed_error_t *error = ordered;
    ordered = error->next;
    if (state->observed_count == INT64_MAX)
      rinzu_failure_fatal();
    error->identity = ++state->observed_count;
    error->next = state->observed;
    state->observed = error;
  }
}
static inline void rinzu_async_release(rinzu_async_future_t *future) {
  rinzu_async_runtime_t *runtime = future->runtime;
  rinzu_async_owner(runtime);
  if (future->operation == RINZU_ASYNC_COMPLETION && future->executed) {
    rinzu_async_lock(runtime);
    if (!atomic_load_explicit(&future->physical_done, memory_order_acquire))
      rinzu_failure_fatal();
    if (future->completion_next != future) {
      rinzu_async_future_t **native = &runtime->native_completions;
      while (*native && *native != future)
        native = &(*native)->completion_next;
      if (!*native)
        rinzu_failure_fatal();
      *native = future->completion_next;
    }
    rinzu_async_unlock(runtime);
  }
  if (!runtime->records || !runtime->future_capacity || future->admitted || future->ready_waiters ||
      future->task_context || future->native_board || (future->started && !future->joined))
    rinzu_failure_fatal();
  size_t bucket = rinzu_identity_bucket(future->identity, runtime->future_capacity);
  rinzu_async_future_t **link = &runtime->future_index[bucket];
  while (*link && *link != future)
    link = &(*link)->index_next;
  if (!*link ||
      (future->previous ? future->previous->next != future : runtime->futures != future) ||
      (future->next && future->next->previous != future))
    rinzu_failure_fatal();
  *link = future->index_next;
  if (future->previous)
    future->previous->next = future->next;
  else
    runtime->futures = future->next;
  if (future->next)
    future->next->previous = future->previous;
  runtime->records -= 1;
  if (runtime->future_capacity > 32 && runtime->records < runtime->future_capacity / 8)
    rinzu_async_index_resize(runtime, runtime->future_capacity / 2);
  rinzu_failure_state_t observed = {0};
  observed.observed = future->observed;
  rinzu_failure_dispose(&observed);
  if (future->readiness) {
    /* Unsubmitted/rejected futures have no physical publisher to wait for. */
    if (!future->settled)
      rinzu_ready_receive(future->readiness);
    rinzu_ready_dispose(future->readiness);
  }
  free(future->arguments);
  free(future->input);
  free(future->result);
  free(future->native_payload);
  rinzu_failure_release(future->failure);
  if (future->accounting_pending) {
    if (!runtime->pending_operations)
      rinzu_failure_fatal();
    --runtime->pending_operations;
    rinzu_resource_release(RINZU_RESOURCE_PENDING_OPERATION, 1);
  }
  if (future->main_admitted) {
    if (!runtime->live_main_tasks)
      rinzu_failure_fatal();
    --runtime->live_main_tasks;
  }
  if (future->accounting_task)
    rinzu_resource_release(RINZU_RESOURCE_TASK, 1);
  rinzu_resource_release(RINZU_RESOURCE_FUTURE, 1);
  rinzu_async_control_release(runtime, future, sizeof(*future));
}
static inline void rinzu_async_stop_completion_reactor(rinzu_async_runtime_t *runtime) {
  rinzu_async_owner(runtime);
  rinzu_async_lock(runtime);
  runtime->reactor_stopping = 1;
  pthread_cond_broadcast(&runtime->completion_changed);
  rinzu_async_unlock(runtime);
  if (runtime->reactor_started) {
    if (pthread_join(runtime->completion_reactor, NULL))
      rinzu_failure_fatal();
    runtime->reactor_started = 0;
  }
}
/* Terminal Owner operation. Physical publication precedes the publisher's wake
   callback: done alone does not authorize freeing its native target. All real
   helpers must be joined and native carriers physically complete before this
   joins the common reactor and clears the immutable wake configuration. */
static inline void rinzu_async_clear_completion_wake(rinzu_async_runtime_t *runtime) {
  rinzu_async_owner(runtime);
  for (rinzu_async_future_t *future = runtime->futures; future; future = future->next)
    if ((future->started && !future->joined) ||
        (future->operation == RINZU_ASYNC_COMPLETION && future->executed &&
         !atomic_load_explicit(&future->physical_done, memory_order_acquire)))
      rinzu_failure_fatal();
  rinzu_async_stop_completion_reactor(runtime);
  if (runtime->native_wake) {
    rinzu_async_native_wake_t *target = runtime->native_wake;
    if (pthread_mutex_lock(&target->gate))
      rinzu_failure_fatal();
    target->runtime = NULL;
    if (pthread_mutex_unlock(&target->gate))
      rinzu_failure_fatal();
    runtime->native_wake = NULL;
    target->header.release(&target->header);
  }
  runtime->completion_wake = NULL;
  runtime->wake_context = NULL;
}
static inline void rinzu_async_dispose(rinzu_async_runtime_t *runtime) {
  rinzu_async_owner(runtime);
  rinzu_async_stop_completion_reactor(runtime);
  while (runtime->futures) {
    // release unlinks the head before freeing it.
    // NOLINTNEXTLINE(clang-analyzer-unix.Malloc)
    rinzu_async_join(runtime->futures);
    if (runtime->futures->admitted || runtime->futures->started)
      rinzu_async_materialize(runtime->futures);
    rinzu_async_release(runtime->futures);
  }
  rinzu_async_clear_completion_wake(runtime);
  if (runtime->records || runtime->domains || runtime->in_flight || runtime->transport_bytes ||
      pthread_cond_destroy(&runtime->completion_changed) || pthread_mutex_destroy(&runtime->gate))
    rinzu_failure_fatal();
  while (runtime->credits) {
    rinzu_async_credit_t *credit = runtime->credits;
    runtime->credits = credit->next;
    if (!credit->released_at)
      rinzu_failure_fatal();
    free(credit);
  }
  runtime->credits_tail = NULL;
  free(runtime->future_index);
  runtime->future_index = NULL;
  runtime->future_capacity = 0;
  rinzu_async_control_dispose(runtime);
}
#endif
