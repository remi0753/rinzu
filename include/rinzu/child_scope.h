#ifndef RINZU_CHILD_SCOPE_H
#define RINZU_CHILD_SCOPE_H
#include "rinzu/failure.h"
#include "rinzu/identity_index.h"
#include "rinzu/thread_identity.h"

enum {
  RINZU_CHILD_OK,
  RINZU_CHILD_CLOSED,
  RINZU_CHILD_DUPLICATE,
  RINZU_CHILD_MISSING,
  RINZU_CHILD_COMPLETE,
  RINZU_CHILD_PENDING,
  RINZU_CHILD_NOT_CLOSING
};
typedef struct rinzu_child {
  uint64_t identity;
  int completed, observed, expected_control;
  rinzu_failure_t failure;
  struct rinzu_child *next, *previous, *index_next;
} rinzu_child_t;
typedef struct rinzu_child_error {
  uint64_t origin;
  int has_origin;
  rinzu_failure_t failure;
  struct rinzu_child_error *next;
} rinzu_child_error_t;
typedef struct {
  pthread_t owner;
  uint64_t task_identity, admissions;
  uint64_t *admission_counter;
  rinzu_child_t *children, *children_tail;
  rinzu_child_t **child_index;
  size_t child_count, child_capacity;
  int closing, finished;
  rinzu_failure_t outcome;
  int collect_all;
  rinzu_child_error_t *errors, *errors_tail;
} rinzu_child_scope_t;
static inline void rinzu_child_init(rinzu_child_scope_t *scope) {
  *scope = (rinzu_child_scope_t){0};
  scope->owner = pthread_self();
}
static inline void rinzu_child_init_all(rinzu_child_scope_t *scope) {
  rinzu_child_init(scope);
  scope->collect_all = 1;
}
static inline void rinzu_child_owner(const rinzu_child_scope_t *scope) {
  if (!rinzu_thread_equal(scope->owner, pthread_self()))
    rinzu_failure_fatal();
}
static inline rinzu_child_t *rinzu_child_find(rinzu_child_scope_t *scope, uint64_t identity) {
  if (!scope->child_capacity)
    return NULL;
  size_t bucket = rinzu_identity_bucket(identity, scope->child_capacity);
  for (rinzu_child_t *child = scope->child_index[bucket]; child; child = child->index_next)
    if (child->identity == identity)
      return child;
  return NULL;
}
static inline void rinzu_child_index_resize(rinzu_child_scope_t *scope, size_t capacity) {
  if (capacity < 8 || (capacity & (capacity - 1)) ||
      capacity > SIZE_MAX / sizeof(*scope->child_index))
    rinzu_failure_fatal();
  rinzu_child_t **index = rinzu_failure_allocate(capacity * sizeof(*index));
  for (rinzu_child_t *child = scope->children; child; child = child->next) {
    size_t bucket = rinzu_identity_bucket(child->identity, capacity);
    child->index_next = index[bucket];
    index[bucket] = child;
  }
  free(scope->child_index);
  scope->child_index = index;
  scope->child_capacity = capacity;
}
static inline void rinzu_child_index_reserve(rinzu_child_scope_t *scope) {
  if (scope->child_count == SIZE_MAX)
    rinzu_failure_fatal();
  if (scope->child_count >= scope->child_capacity) {
    if (scope->child_capacity > SIZE_MAX / 2)
      rinzu_failure_fatal();
    rinzu_child_index_resize(scope, scope->child_capacity ? scope->child_capacity * 2 : 8);
  }
}
static inline void rinzu_child_index_insert(rinzu_child_scope_t *scope, rinzu_child_t *child) {
  size_t bucket = rinzu_identity_bucket(child->identity, scope->child_capacity);
  child->index_next = scope->child_index[bucket];
  scope->child_index[bucket] = child;
  ++scope->child_count;
}
static inline void rinzu_child_unlink(rinzu_child_scope_t *scope, rinzu_child_t *child) {
  if (!scope->child_capacity || !scope->child_count)
    rinzu_failure_fatal();
  size_t bucket = rinzu_identity_bucket(child->identity, scope->child_capacity);
  rinzu_child_t **link = &scope->child_index[bucket];
  while (*link && *link != child)
    link = &(*link)->index_next;
  if (!*link || (child->previous ? child->previous->next != child : scope->children != child) ||
      (child->next ? child->next->previous != child : scope->children_tail != child))
    rinzu_failure_fatal();
  *link = child->index_next;
  if (child->previous) child->previous->next = child->next;
  else scope->children = child->next;
  if (child->next) child->next->previous = child->previous;
  else scope->children_tail = child->previous;
  child->previous = child->next = child->index_next = NULL;
  --scope->child_count;
}
static inline void rinzu_child_index_dispose(rinzu_child_scope_t *scope) {
  free(scope->child_index);
  scope->child_index = NULL;
  scope->child_count = scope->child_capacity = 0;
}
static inline int rinzu_child_admit_with_lease(rinzu_child_scope_t *scope, uint64_t identity,
                                               int callback) {
  rinzu_child_owner(scope);
  if (scope->finished || (scope->closing && !callback))
    return RINZU_CHILD_CLOSED;
  rinzu_child_t **link = &scope->children;
  /* Runtime identities are increasing. Preserve sorted order for generic
     callers too, without searching from the head for every normal admission. */
  if (scope->children_tail && scope->children_tail->identity < identity)
    link = &scope->children_tail->next;
  while (*link && (*link)->identity < identity)
    link = &(*link)->next;
  if (*link && (*link)->identity == identity)
    return RINZU_CHILD_DUPLICATE;
  rinzu_child_index_reserve(scope);
  rinzu_child_t *child = rinzu_failure_allocate(sizeof(*child));
  child->identity = identity;
  child->next = *link;
  child->previous = *link ? (*link)->previous : scope->children_tail;
  if (*link) (*link)->previous = child;
  *link = child;
  if (!child->next)
    scope->children_tail = child;
  rinzu_child_index_insert(scope, child);
  return RINZU_CHILD_OK;
}
static inline int rinzu_child_admit(rinzu_child_scope_t *scope, uint64_t identity) {
  return rinzu_child_admit_with_lease(scope, identity, 0);
}
/* Owner-only transfer; preserves completion, observation and control records.
   The runtime supervisor may already be draining after Main returns. */
static inline int rinzu_child_handoff(rinzu_child_scope_t *scope, rinzu_child_scope_t *supervisor) {
  rinzu_child_owner(scope);
  rinzu_child_owner(supervisor);
  if (scope == supervisor || scope->closing || scope->finished || supervisor->finished)
    return RINZU_CHILD_CLOSED;
  rinzu_child_t **link = &supervisor->children;
  if (scope->children && supervisor->children_tail &&
      supervisor->children_tail->identity < scope->children->identity)
    link = &supervisor->children_tail->next;
  while (scope->children) {
    rinzu_child_t *child = scope->children;
    while (*link && (*link)->identity < child->identity)
      link = &(*link)->next;
    if (*link && (*link)->identity == child->identity)
      rinzu_failure_fatal();
    rinzu_child_unlink(scope, child);
    rinzu_child_index_reserve(supervisor);
    child->previous = *link ? (*link)->previous : supervisor->children_tail;
    child->next = *link;
    if (*link) (*link)->previous = child;
    *link = child;
    if (!child->next)
      supervisor->children_tail = child;
    rinzu_child_index_insert(supervisor, child);
    link = &child->next;
  }
  scope->children_tail = NULL;
  return RINZU_CHILD_OK;
}
static inline int rinzu_child_complete(rinzu_child_scope_t *scope, uint64_t identity,
                                       const rinzu_failure_t *failure) {
  rinzu_child_owner(scope);
  rinzu_child_t *child = rinzu_child_find(scope, identity);
  if (!child)
    return RINZU_CHILD_MISSING;
  if (child->completed)
    return RINZU_CHILD_COMPLETE;
  if (failure)
    child->failure = rinzu_failure_copy(failure);
  child->completed = 1;
  return RINZU_CHILD_OK;
}
static inline int rinzu_child_observe(rinzu_child_scope_t *scope, uint64_t identity) {
  rinzu_child_owner(scope);
  rinzu_child_t *child = rinzu_child_find(scope, identity);
  if (!child)
    return RINZU_CHILD_MISSING;
  if (!child->completed)
    return RINZU_CHILD_PENDING;
  child->observed = 1;
  return RINZU_CHILD_OK;
}
static inline int rinzu_child_expect_control(rinzu_child_scope_t *scope, uint64_t identity) {
  rinzu_child_owner(scope);
  rinzu_child_t *child = rinzu_child_find(scope, identity);
  if (!child)
    return RINZU_CHILD_MISSING;
  child->expected_control = 1;
  return RINZU_CHILD_OK;
}
/* Roll back an unsubmitted reservation before its Future is exposed. */
static inline int rinzu_child_withdraw_admission(rinzu_child_scope_t *scope, uint64_t identity) {
  rinzu_child_owner(scope);
  rinzu_child_t *child = rinzu_child_find(scope, identity);
  if (!child)
    return RINZU_CHILD_MISSING;
  if (child->completed)
    return RINZU_CHILD_COMPLETE;
  rinzu_child_unlink(scope, child);
  free(child);
  return RINZU_CHILD_OK;
}
/* Retire only the completed, observed parent ledger. Callers independently
   retain the Future, immutable result/failure and physical resources until
   their own consume/GC/release rules permit retirement. */
static inline int rinzu_child_retire_observed(rinzu_child_scope_t *scope, uint64_t identity) {
  rinzu_child_owner(scope);
  rinzu_child_t *child = rinzu_child_find(scope, identity);
  if (!child)
    return RINZU_CHILD_MISSING;
  if (!child->completed || !child->observed)
    return RINZU_CHILD_PENDING;
  rinzu_child_unlink(scope, child);
  rinzu_failure_release(child->failure);
  free(child);
  return RINZU_CHILD_OK;
}
/* A dead handle is not an observation of a failure. Retain unhandled outcomes. */
static inline int rinzu_child_retire_unreferenced(rinzu_child_scope_t *scope,uint64_t identity) {
  rinzu_child_owner(scope);
  rinzu_child_t *child=rinzu_child_find(scope,identity);
  if (!child) rinzu_failure_fatal();
  if (!child->completed || (child->failure.primary && !child->observed &&
      !(child->expected_control && !child->failure.head &&
        (child->failure.primary==2 || child->failure.primary==4)))) return 0;
  rinzu_child_unlink(scope,child);
  rinzu_failure_release(child->failure);free(child);return 1;
}
static inline int rinzu_child_begin_close(rinzu_child_scope_t *scope,
                                          const rinzu_failure_t *outcome) {
  rinzu_child_owner(scope);
  if (scope->closing)
    return RINZU_CHILD_CLOSED;
  if (outcome)
    scope->outcome = rinzu_failure_copy(outcome);
  scope->closing = 1;
  return RINZU_CHILD_OK;
}
static inline void rinzu_child_append(rinzu_failure_t *outcome, int64_t cause,
                                      rinzu_source_position_t source) {
  rinzu_diagnostic_node_t *node = rinzu_failure_allocate(sizeof(*node));
  node->cause = cause;
  node->source = source;
  if (outcome->tail)
    outcome->tail->next = node;
  else
    outcome->head = node;
  outcome->tail = node;
}
static inline void rinzu_child_append_error(rinzu_failure_t *outcome, int64_t cause,
                                             rinzu_source_position_t source, rinzu_error_reference_t *error) {
  rinzu_child_append(outcome, cause, source);
  outcome->tail->error = rinzu_error_reference_copy(error);
}
static inline void rinzu_child_append_traced_error(rinzu_failure_t *outcome, int64_t cause,
    rinzu_source_position_t source, rinzu_error_reference_t *error, rinzu_error_trace_t *trace) {
  rinzu_child_append_error(outcome, cause, source, error);
  outcome->tail->trace = rinzu_error_trace_copy(trace);
}
static inline int rinzu_child_cancels_on_failure(const rinzu_child_scope_t *scope,
                                                 const rinzu_failure_t *failure) {
  rinzu_failure_state_t state = {.current = *failure};
  return !scope->collect_all || !rinzu_failure_recoverable(&state);
}
static inline int rinzu_child_record_error(rinzu_child_scope_t *scope, uint64_t origin,
                                           int has_origin, const rinzu_failure_t *failure) {
  rinzu_child_owner(scope);
  if (scope->finished || !scope->collect_all)
    return RINZU_CHILD_CLOSED;
  rinzu_failure_state_t state = {.current = *failure};
  if (!rinzu_failure_recoverable(&state))
    rinzu_failure_fatal();
  for (rinzu_child_error_t *error = scope->errors; error; error = error->next)
    if (has_origin && error->has_origin && error->origin == origin)
      return RINZU_CHILD_OK;
  rinzu_child_error_t *error = rinzu_failure_allocate(sizeof(*error));
  error->origin = origin;
  error->has_origin = has_origin;
  error->failure = rinzu_failure_copy(failure);
  if (scope->errors_tail)
    scope->errors_tail->next = error;
  else
    scope->errors = error;
  scope->errors_tail = error;
  return RINZU_CHILD_OK;
}
static inline void rinzu_child_merge(rinzu_failure_t *outcome, const rinzu_failure_t *failure) {
  if (!outcome->primary) {
    *outcome = rinzu_failure_copy(failure);
    return;
  }
  rinzu_child_append_traced_error(outcome, failure->primary, failure->primary_source, failure->primary_error, failure->primary_trace);
  for (rinzu_diagnostic_node_t *node = failure->head; node; node = node->next)
    rinzu_child_append_traced_error(outcome, node->cause, node->source, node->error, node->trace);
}
static inline int rinzu_child_finish_all(rinzu_child_scope_t *scope) {
  if (scope->outcome.primary && !rinzu_child_cancels_on_failure(scope, &scope->outcome)) {
    if (rinzu_child_record_error(scope, 0, 0, &scope->outcome))
      rinzu_failure_fatal();
    rinzu_failure_release(scope->outcome);
    scope->outcome = (rinzu_failure_t){0};
  }
  while (scope->children) {
    rinzu_child_t *child = scope->children;
    scope->children = child->next;
    rinzu_failure_t failure = child->failure;
    if (!child->observed &&
        ((child->expected_control && failure.primary == 2) ||
         (scope->outcome.primary &&
          (failure.primary == 2 || (scope->outcome.primary == 4 && failure.primary == 4))))) {
      if (failure.head) {
        failure.primary = failure.head->cause;
        failure.primary_source = failure.head->source;
        failure.primary_error = failure.head->error;
        failure.primary_trace = failure.head->trace;
        failure.head = failure.head->next;
      } else
        failure.primary = 0;
    }
    if (!child->observed && failure.primary) {
      if (!rinzu_child_cancels_on_failure(scope, &failure)) {
        if (rinzu_child_record_error(scope, child->identity, 1, &failure))
          rinzu_failure_fatal();
      } else
        rinzu_child_merge(&scope->outcome, &failure);
    }
    rinzu_failure_release(child->failure);
    free(child);
  }
  for (rinzu_child_error_t *error = scope->errors; error; error = error->next)
    rinzu_child_merge(&scope->outcome, &error->failure);
  scope->children_tail = NULL;
  rinzu_child_index_dispose(scope);
  scope->finished = 1;
  return RINZU_CHILD_OK;
}
static inline int rinzu_child_interrupt_close(rinzu_child_scope_t *scope,
                                              rinzu_source_position_t source, int64_t cause) {
  rinzu_child_owner(scope);
  if (cause != 2 && cause != 4)
    rinzu_failure_fatal();
  if (!scope->closing)
    return RINZU_CHILD_NOT_CLOSING;
  if (scope->finished)
    return RINZU_CHILD_CLOSED;
  if (!scope->outcome.primary) {
    scope->outcome.primary = cause;
    scope->outcome.primary_source = source;
  }
  return RINZU_CHILD_OK;
}
static inline int rinzu_child_cancel_close(rinzu_child_scope_t *scope,
                                           rinzu_source_position_t source) {
  return rinzu_child_interrupt_close(scope, source, 2);
}
static inline int rinzu_child_shutdown_close(rinzu_child_scope_t *scope,
                                             rinzu_source_position_t source) {
  return rinzu_child_interrupt_close(scope, source, 4);
}
static inline int rinzu_child_finish_close(rinzu_child_scope_t *scope) {
  rinzu_child_owner(scope);
  if (!scope->closing)
    return RINZU_CHILD_NOT_CLOSING;
  if (scope->finished)
    return RINZU_CHILD_OK;
  for (rinzu_child_t *child = scope->children; child; child = child->next)
    if (!child->completed)
      return RINZU_CHILD_PENDING;
  if (scope->collect_all)
    return rinzu_child_finish_all(scope);
  int parent_failed = !!scope->outcome.primary;
  while (scope->children) {
    rinzu_child_t *child = scope->children;
    scope->children = child->next;
    if (!child->observed && child->expected_control && child->failure.primary == 2) {
      if (child->failure.head) {
        if (!scope->outcome.primary) {
          scope->outcome.primary = 11;
          scope->outcome.primary_source = child->failure.head->source;
        }
        for (rinzu_diagnostic_node_t *node = child->failure.head; node; node = node->next)
          rinzu_child_append_traced_error(&scope->outcome, node->cause, node->source, node->error, node->trace);
      }
    } else if (!child->observed && child->failure.primary) {
      if (!parent_failed && !scope->outcome.primary) {
        rinzu_failure_state_t failure = {.current = child->failure};
        if (!rinzu_failure_recoverable(&failure)) {
          scope->outcome = rinzu_failure_copy(&child->failure);
          rinzu_failure_release(child->failure);
          free(child);
          continue;
        }
        scope->outcome.primary = 11;
        scope->outcome.primary_source = child->failure.primary_source;
      }
      if (!parent_failed || !(child->failure.primary == 2 ||
                              (scope->outcome.primary == 4 && child->failure.primary == 4)))
        rinzu_child_append_traced_error(&scope->outcome, child->failure.primary, child->failure.primary_source, child->failure.primary_error, child->failure.primary_trace);
      for (rinzu_diagnostic_node_t *node = child->failure.head; node; node = node->next)
        rinzu_child_append_traced_error(&scope->outcome, node->cause, node->source, node->error, node->trace);
    }
    rinzu_failure_release(child->failure);
    free(child);
  }
  scope->children_tail = NULL;
  rinzu_child_index_dispose(scope);
  scope->finished = 1;
  return RINZU_CHILD_OK;
}
static inline void rinzu_child_dispose(rinzu_child_scope_t *scope) {
  rinzu_child_owner(scope);
  if (!scope->finished || scope->children || scope->children_tail || scope->child_count ||
      scope->child_index || scope->child_capacity)
    rinzu_failure_fatal();
  rinzu_failure_release(scope->outcome);
  while (scope->errors) {
    rinzu_child_error_t *error = scope->errors;
    scope->errors = error->next;
    rinzu_failure_release(error->failure);
    free(error);
  }
  *scope = (rinzu_child_scope_t){0};
}
#endif
