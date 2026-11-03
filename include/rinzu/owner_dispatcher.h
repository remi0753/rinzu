#ifndef RINZU_OWNER_DISPATCHER_H
#define RINZU_OWNER_DISPATCHER_H
#include "rinzu/foreign_queue.h"

/* Private runtime hooks. Lane 0 is cleanup/control, lane 1 is a single ready
   task/I/O activation. The dispatcher owns the foreign FIFO lane itself. */
typedef struct rinzu_owner_dispatcher rinzu_owner_dispatcher_t;
typedef int (*rinzu_owner_step_t)(void *, unsigned);
typedef int (*rinzu_owner_ready_t)(void *, unsigned);
typedef int (*rinzu_owner_enter_t)(void *);
typedef struct {
  uint64_t wakes, signals, turns, activations, foreign, managed, rearms, deferred;
  size_t max_activations;
} rinzu_owner_dispatcher_stats_t;
rinzu_owner_dispatcher_t *rinzu_owner_dispatcher_new(rinzu_foreign_queue_t *, rinzu_owner_step_t,
                                                     rinzu_owner_ready_t, rinzu_owner_enter_t,
                                                     void *);
void rinzu_owner_dispatcher_set_hooks(rinzu_owner_dispatcher_t *, rinzu_owner_step_t,
                                      rinzu_owner_ready_t, rinzu_owner_enter_t, void *);
void rinzu_owner_dispatcher_wake(void *);
void rinzu_owner_dispatcher_pump(rinzu_owner_dispatcher_t *);
rinzu_owner_dispatcher_stats_t rinzu_owner_dispatcher_stats(rinzu_owner_dispatcher_t *);
void rinzu_owner_dispatcher_dispose(rinzu_owner_dispatcher_t *);
#ifdef RINZU_DISPATCHER_TESTING
void rinzu_owner_dispatcher_test_empty_hook(rinzu_foreign_notify_t, void *);
void rinzu_owner_dispatcher_test_return_after_published(rinzu_owner_dispatcher_t *, size_t);
#endif
#endif
