#ifndef RINZU_FOREIGN_QUEUE_H
#define RINZU_FOREIGN_QUEUE_H
#include <errno.h>
#include "rinzu/thread_identity.h"
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

/* Private admitted-invocation transport. These words are ABI bits/pointers,
   never managed values. Signature/progress/admission checks precede creation. */
typedef struct rinzu_foreign_queue rinzu_foreign_queue_t;
typedef struct rinzu_foreign_record rinzu_foreign_record_t;
typedef void (*rinzu_foreign_notify_t)(void *);
typedef struct {
  size_t live, peak_live, bytes, peak_bytes, queued, peak_queued, wake_pins;
  uint64_t created, published, received, retired;
} rinzu_foreign_stats_t;
struct rinzu_foreign_queue {
  pthread_mutex_t gate;
  pthread_t owner;
  rinzu_foreign_record_t *head, *tail;
  rinzu_foreign_notify_t wake;
  void *wake_context;
  rinzu_foreign_stats_t stats;
  uint64_t loop_generation;
  size_t bounded_depth;
  unsigned main_domain;
  size_t runloop_depth;
  unsigned dispatcher_attached, runloop_allowed;
};
enum {
  RINZU_FOREIGN_PREPARED,
  RINZU_FOREIGN_QUEUED,
  RINZU_FOREIGN_EXECUTING,
  RINZU_FOREIGN_COMPLETED,
  RINZU_FOREIGN_RECEIVED
};
struct rinzu_foreign_record {
  _Atomic size_t references;
  pthread_mutex_t result_gate;
  pthread_cond_t result_changed;
  rinzu_foreign_queue_t *queue;
  rinzu_foreign_record_t *next;
  rinzu_foreign_notify_t receipt;
  void *receipt_context;
  rinzu_foreign_notify_t ready_received, ready_dispose;
  void *ready_context;
  pthread_t caller;
  uint64_t identity, sequence, callback, generation, result;
  struct timespec admitted_at, dequeued_at, completed_at;
  size_t count, bytes;
  unsigned state, admission_settled;
  uint64_t words[];
};
#ifdef RINZU_FOREIGN_TESTING
static _Thread_local size_t rinzu_foreign_held_locks;
#endif
static inline void rinzu_foreign_lock(pthread_mutex_t *lock) {
  if (pthread_mutex_lock(lock))
    _Exit(86);
#ifdef RINZU_FOREIGN_TESTING
  rinzu_foreign_held_locks++;
#endif
}
static inline void rinzu_foreign_unlock(pthread_mutex_t *lock) {
#ifdef RINZU_FOREIGN_TESTING
  if (!rinzu_foreign_held_locks)
    _Exit(86);
  rinzu_foreign_held_locks--;
#endif
  if (pthread_mutex_unlock(lock))
    _Exit(86);
}
static inline void rinzu_foreign_sync(int status) {
  if (status)
    _Exit(status == ENOMEM || status == EAGAIN ? 71 : 86);
}
static inline void *rinzu_foreign_allocate(size_t bytes) {
  void *value = calloc(1, bytes);
  if (!value)
    _Exit(71);
  return value;
}
static inline void rinzu_foreign_owner(rinzu_foreign_queue_t *queue) {
  if (!queue || !rinzu_thread_equal(queue->owner, pthread_self()))
    _Exit(86);
}
static inline void rinzu_foreign_time(struct timespec *time) {
#ifdef __APPLE__
  /* Darwin's CLOCK_MONOTONIC is calendar/boottime based. Use the same raw,
     sleep-inclusive elapsed clock as libc++ steady_clock for native intervals. */
  if (clock_gettime(CLOCK_MONOTONIC_RAW, time))
#else
  if (clock_gettime(CLOCK_MONOTONIC, time))
#endif
    _Exit(86);
}
static inline void rinzu_foreign_notify_pinned(rinzu_foreign_queue_t *queue,
                                               rinzu_foreign_notify_t wake, void *context) {
  if (!wake)
    return;
  wake(context);
  rinzu_foreign_lock(&queue->gate);
  if (!queue->stats.wake_pins)
    _Exit(86);
  queue->stats.wake_pins--;
  rinzu_foreign_unlock(&queue->gate);
}
static inline void rinzu_foreign_wake(rinzu_foreign_queue_t *queue) {
  rinzu_foreign_lock(&queue->gate);
  rinzu_foreign_notify_t wake = queue->wake;
  void *context = queue->wake_context;
  if (wake) {
    if (queue->stats.wake_pins == SIZE_MAX)
      _Exit(71);
    queue->stats.wake_pins++;
  }
  rinzu_foreign_unlock(&queue->gate);
  rinzu_foreign_notify_pinned(queue, wake, context);
}
static inline void rinzu_foreign_attach_main(rinzu_foreign_queue_t *queue) {
  rinzu_foreign_owner(queue);
  rinzu_foreign_lock(&queue->gate);
  if (queue->main_domain || queue->loop_generation || queue->bounded_depth || queue->stats.live)
    _Exit(86);
  queue->main_domain = 1;
  rinzu_foreign_unlock(&queue->gate);
}
static inline void rinzu_foreign_loop_begin(rinzu_foreign_queue_t *queue, uint64_t generation) {
  rinzu_foreign_owner(queue);
  rinzu_foreign_lock(&queue->gate);
  if (!generation || !queue->main_domain || queue->loop_generation || queue->bounded_depth)
    _Exit(86);
  queue->loop_generation = generation;
  rinzu_foreign_unlock(&queue->gate);
}
static inline void rinzu_foreign_loop_end(rinzu_foreign_queue_t *queue, uint64_t generation) {
  rinzu_foreign_owner(queue);
  rinzu_foreign_lock(&queue->gate);
  if (!generation || queue->loop_generation != generation || queue->bounded_depth)
    _Exit(86);
  queue->loop_generation = 0;
  rinzu_foreign_unlock(&queue->gate);
}
static inline void rinzu_foreign_bounded_enter(rinzu_foreign_queue_t *queue) {
  rinzu_foreign_owner(queue);
  rinzu_foreign_lock(&queue->gate);
  if (queue->bounded_depth == SIZE_MAX)
    _Exit(71);
  queue->bounded_depth++;
  rinzu_foreign_unlock(&queue->gate);
}
static inline void rinzu_foreign_bounded_leave(rinzu_foreign_queue_t *queue) {
  rinzu_foreign_owner(queue);
  rinzu_foreign_lock(&queue->gate);
  if (!queue->bounded_depth)
    _Exit(86);
  queue->bounded_depth--;
  rinzu_foreign_unlock(&queue->gate);
}
enum {
  RINZU_FOREIGN_ALLOWED,
  RINZU_FOREIGN_NO_ORIGIN,
  RINZU_FOREIGN_INACTIVE_ORIGIN,
  RINZU_FOREIGN_BOUNDED_WAIT,
  RINZU_FOREIGN_UNVERIFIED_LOOP
};
static inline unsigned rinzu_foreign_admission_status(rinzu_foreign_queue_t *queue,
                                                      uint64_t generation) {
  if (!queue)
    return RINZU_FOREIGN_NO_ORIGIN;
  rinzu_foreign_lock(&queue->gate);
  unsigned status =
      !queue->main_domain                                     ? RINZU_FOREIGN_NO_ORIGIN
      : (!generation || queue->loop_generation != generation) ? RINZU_FOREIGN_INACTIVE_ORIGIN
      : queue->bounded_depth                                  ? RINZU_FOREIGN_BOUNDED_WAIT
      : (queue->dispatcher_attached && (queue->runloop_depth != 1 || !queue->runloop_allowed))
          ? RINZU_FOREIGN_UNVERIFIED_LOOP
          : RINZU_FOREIGN_ALLOWED;
  rinzu_foreign_unlock(&queue->gate);
  return status;
}
static inline int rinzu_foreign_can_admit(rinzu_foreign_queue_t *queue, uint64_t generation) {
  return rinzu_foreign_admission_status(queue, generation) == RINZU_FOREIGN_ALLOWED;
}
static inline rinzu_foreign_queue_t *rinzu_foreign_new(rinzu_foreign_notify_t wake, void *context) {
  rinzu_foreign_queue_t *queue = rinzu_foreign_allocate(sizeof(*queue));
  rinzu_foreign_sync(pthread_mutex_init(&queue->gate, NULL));
  queue->owner = pthread_self();
  queue->wake = wake;
  queue->wake_context = context;
  return queue;
}
static inline rinzu_foreign_stats_t rinzu_foreign_stats(rinzu_foreign_queue_t *queue) {
  if (!queue)
    _Exit(86);
  rinzu_foreign_lock(&queue->gate);
  rinzu_foreign_stats_t stats = queue->stats;
  rinzu_foreign_unlock(&queue->gate);
  return stats;
}
static inline void rinzu_foreign_dispose(rinzu_foreign_queue_t *queue) {
  rinzu_foreign_owner(queue);
  rinzu_foreign_lock(&queue->gate);
  if (queue->head || queue->tail || queue->stats.live || queue->stats.bytes ||
      queue->stats.queued || queue->stats.wake_pins || queue->loop_generation ||
      queue->bounded_depth || queue->dispatcher_attached)
    _Exit(86);
  rinzu_foreign_unlock(&queue->gate);
  rinzu_foreign_sync(pthread_mutex_destroy(&queue->gate));
  free(queue);
}
static inline rinzu_foreign_record_t *
rinzu_foreign_record_new(rinzu_foreign_queue_t *queue, uint64_t callback, uint64_t generation,
                         const uint64_t *words, size_t count, rinzu_foreign_notify_t receipt,
                         void *context) {
  if (!queue || !callback || !generation || (count && !words) || (receipt && !context))
    _Exit(86);
  if (count > (SIZE_MAX - sizeof(rinzu_foreign_record_t)) / sizeof(uint64_t))
    _Exit(71);
  size_t bytes = sizeof(rinzu_foreign_record_t) + count * sizeof(uint64_t);
  rinzu_foreign_record_t *record = rinzu_foreign_allocate(bytes);
  atomic_init(&record->references, 1);
  rinzu_foreign_sync(pthread_mutex_init(&record->result_gate, NULL));
  rinzu_foreign_sync(pthread_cond_init(&record->result_changed, NULL));
  record->queue = queue;
  record->callback = callback;
  record->generation = generation;
  record->caller = pthread_self();
  record->receipt = receipt;
  record->receipt_context = context;
  record->count = count;
  record->bytes = bytes;
  if (count)
    memcpy(record->words, words, count * sizeof(*words));
  rinzu_foreign_time(&record->admitted_at);
  rinzu_foreign_lock(&queue->gate);
  if (queue->stats.created == UINT64_MAX || queue->stats.live == SIZE_MAX ||
      bytes > SIZE_MAX - queue->stats.bytes)
    _Exit(71);
  record->identity = ++queue->stats.created;
  queue->stats.live++;
  queue->stats.bytes += bytes;
  if (queue->stats.live > queue->stats.peak_live)
    queue->stats.peak_live = queue->stats.live;
  if (queue->stats.bytes > queue->stats.peak_bytes)
    queue->stats.peak_bytes = queue->stats.bytes;
  rinzu_foreign_unlock(&queue->gate);
  return record;
}
static inline void rinzu_foreign_release(rinzu_foreign_record_t *record) {
  if (!record)
    _Exit(86);
  size_t before = atomic_fetch_sub_explicit(&record->references, 1, memory_order_acq_rel);
  if (!before)
    _Exit(86);
  if (before != 1)
    return;
  if (record->state != RINZU_FOREIGN_RECEIVED || !record->admission_settled)
    _Exit(86);
  rinzu_foreign_queue_t *queue = record->queue;
  size_t bytes = record->bytes;
  rinzu_foreign_sync(pthread_cond_destroy(&record->result_changed));
  rinzu_foreign_sync(pthread_mutex_destroy(&record->result_gate));
  if (record->ready_dispose)
    record->ready_dispose(record->ready_context);
  free(record);
  rinzu_foreign_lock(&queue->gate);
  if (!queue->stats.live || queue->stats.bytes < bytes || queue->stats.retired == UINT64_MAX)
    _Exit(86);
  queue->stats.live--;
  queue->stats.bytes -= bytes;
  queue->stats.retired++;
  rinzu_foreign_notify_t wake = queue->wake;
  void *context = queue->wake_context;
  if (wake) {
    if (queue->stats.wake_pins == SIZE_MAX)
      _Exit(71);
    queue->stats.wake_pins++;
  }
  rinzu_foreign_unlock(&queue->gate);
  /* Final reclamation can follow result receipt on the other thread. */
  rinzu_foreign_notify_pinned(queue, wake, context);
}
static inline void rinzu_foreign_publish(rinzu_foreign_record_t *record) {
  if (!record || !rinzu_thread_equal(record->caller, pthread_self()))
    _Exit(86);
  rinzu_foreign_queue_t *queue = record->queue;
  rinzu_foreign_lock(&record->result_gate);
  if (record->state != RINZU_FOREIGN_PREPARED || atomic_load(&record->references) != 1)
    _Exit(86);
  atomic_fetch_add_explicit(&record->references, 1, memory_order_relaxed);
  record->state = RINZU_FOREIGN_QUEUED;
  rinzu_foreign_lock(&queue->gate);
  if (queue->stats.published == UINT64_MAX || queue->stats.queued == SIZE_MAX)
    _Exit(71);
  record->sequence = ++queue->stats.published;
  if (queue->tail)
    queue->tail->next = record;
  else
    queue->head = record;
  queue->tail = record;
  queue->stats.queued++;
  if (queue->stats.queued > queue->stats.peak_queued)
    queue->stats.peak_queued = queue->stats.queued;
  rinzu_foreign_unlock(&queue->gate);
  rinzu_foreign_unlock(&record->result_gate);
  /* No queue, admission or result lock crosses a wake/reentrant owner turn. */
  rinzu_foreign_wake(queue);
}
static inline rinzu_foreign_record_t *rinzu_foreign_pop(rinzu_foreign_queue_t *queue) {
  rinzu_foreign_owner(queue);
  rinzu_foreign_lock(&queue->gate);
  rinzu_foreign_record_t *record = queue->head;
  if (record) {
    queue->head = record->next;
    if (!queue->head)
      queue->tail = NULL;
    record->next = NULL;
    if (!queue->stats.queued)
      _Exit(86);
    queue->stats.queued--;
  }
  rinzu_foreign_unlock(&queue->gate);
  if (record) {
    rinzu_foreign_lock(&record->result_gate);
    if (record->state != RINZU_FOREIGN_QUEUED)
      _Exit(86);
    record->state = RINZU_FOREIGN_EXECUTING;
    rinzu_foreign_unlock(&record->result_gate);
    rinzu_foreign_time(&record->dequeued_at);
    if (record->ready_received)
      record->ready_received(record->ready_context);
  }
  return record;
}
static inline void rinzu_foreign_complete(rinzu_foreign_record_t *record, uint64_t result) {
  if (!record)
    _Exit(86);
  rinzu_foreign_owner(record->queue);
  rinzu_foreign_lock(&record->result_gate);
  if (record->state != RINZU_FOREIGN_EXECUTING)
    _Exit(86);
  record->result = result;
  rinzu_foreign_time(&record->completed_at);
  record->state = RINZU_FOREIGN_COMPLETED;
  rinzu_foreign_sync(pthread_cond_signal(&record->result_changed));
  rinzu_foreign_unlock(&record->result_gate);
}
static inline uint64_t rinzu_foreign_receive_before_receipt(rinzu_foreign_record_t *record,
                                                            rinzu_foreign_notify_t before_receipt,
                                                            void *context) {
  if (!record || !rinzu_thread_equal(record->caller, pthread_self()))
    _Exit(86);
  if (before_receipt && !context)
    _Exit(86);
  rinzu_foreign_lock(&record->result_gate);
  if (record->state == RINZU_FOREIGN_PREPARED || record->state == RINZU_FOREIGN_RECEIVED)
    _Exit(86);
  while (record->state != RINZU_FOREIGN_COMPLETED)
    rinzu_foreign_sync(pthread_cond_wait(&record->result_changed, &record->result_gate));
  uint64_t result = record->result;
  record->state = RINZU_FOREIGN_RECEIVED;
  rinzu_foreign_unlock(&record->result_gate);
  /* Finish caller-owned native observation before receipt permits Origin to
     release the last admitted connection and its diagnostic board. */
  if (before_receipt)
    before_receipt(context);
  if (record->receipt)
    record->receipt(record->receipt_context);
  record->admission_settled = 1;
  rinzu_foreign_queue_t *queue = record->queue;
  rinzu_foreign_lock(&queue->gate);
  if (queue->stats.received == UINT64_MAX)
    _Exit(86);
  queue->stats.received++;
  rinzu_foreign_unlock(&queue->gate);
  rinzu_foreign_wake(queue);
  return result;
}
static inline uint64_t rinzu_foreign_receive(rinzu_foreign_record_t *record) {
  return rinzu_foreign_receive_before_receipt(record, NULL, NULL);
}
#endif
