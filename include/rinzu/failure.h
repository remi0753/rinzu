#ifndef RINZU_FAILURE_H
#define RINZU_FAILURE_H
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include "rinzu/error_trace.h"
#include "rinzu/native_fatal.h"

/* Private managed execution diagnostics. Code 5 is a recoverable conversion Error. */
/* A failure owns a reference; its Domain keeps a weak registry for GC. These
   tokens contain no stack or movable heap address and cannot cross Domains. */
typedef struct rinzu_error_reference {
  size_t references;
  void *owner;
  uint64_t handle;
  struct rinzu_error_reference *next;
} rinzu_error_reference_t;
typedef struct rinzu_diagnostic_node {
  int64_t cause;
  rinzu_source_position_t source;
  rinzu_error_reference_t *error;
  rinzu_error_trace_t *trace;
  struct rinzu_diagnostic_node *next;
} rinzu_diagnostic_node_t;
typedef struct {
  int64_t primary;
  rinzu_source_position_t primary_source;
  rinzu_error_reference_t *primary_error;
  rinzu_error_trace_t *primary_trace;
  rinzu_diagnostic_node_t *head, *tail;
} rinzu_failure_t;
typedef struct rinzu_cleanup_frame {
  rinzu_failure_t incoming;
  uint64_t activation, region;
  unsigned scoped;
  struct rinzu_cleanup_frame *previous;
} rinzu_cleanup_frame_t;
typedef struct rinzu_observed_error {
  uint64_t identity;
  rinzu_failure_t failure;
  struct rinzu_observed_error *next;
} rinzu_observed_error_t;
typedef struct {
  rinzu_failure_t current;
  rinzu_cleanup_frame_t *suspended;
  rinzu_observed_error_t *observed;
  uint64_t depth, observed_count;
  rinzu_error_trace_stack_t *trace_stack;
} rinzu_failure_state_t;
static inline void rinzu_failure_fatal(void) {
  /* No output dependency: even a static write may block on a full stderr pipe. */
  _Exit(70);
}
static inline void *rinzu_failure_allocate(size_t size) {
  void *result = calloc(1, size);
  if (!result)
    rinzu_resource_fatal();
  return result;
}
static inline rinzu_error_reference_t *rinzu_error_reference_copy(rinzu_error_reference_t *error) {
  if (error) {
    if (!error->references || error->references == SIZE_MAX)
      rinzu_failure_fatal();
    error->references++;
  }
  return error;
}
static inline void rinzu_error_reference_release(rinzu_error_reference_t *error) {
  if (!error)
    return;
  if (!error->references)
    rinzu_failure_fatal();
  if (!--error->references && !error->owner)
    free(error);
}
static inline void rinzu_failure_fail(rinzu_failure_state_t *state, int64_t cause,
                                      rinzu_source_position_t source) {
  if (!state->current.primary) {
    state->current.primary = cause;
    state->current.primary_source = source;
    if (cause == 5 || (cause >= 8 && cause <= 15))
      state->current.primary_trace = rinzu_error_trace_stack_capture(state->trace_stack, source);
  }
}
static inline void rinzu_failure_begin(rinzu_failure_state_t *state) {
  rinzu_cleanup_frame_t *frame = rinzu_failure_allocate(sizeof(*frame));
  frame->incoming = state->current;
  frame->previous = state->suspended;
  state->suspended = frame;
  state->current = (rinzu_failure_t){0};
  state->depth += 1;
}
static inline void rinzu_failure_end(rinzu_failure_state_t *state) {
  rinzu_cleanup_frame_t *frame = state->suspended;
  if (!frame || !state->depth)
    rinzu_failure_fatal();
  rinzu_failure_t incoming = frame->incoming;
  if (incoming.primary && state->current.primary) {
    rinzu_diagnostic_node_t *node = rinzu_failure_allocate(sizeof(*node));
    node->cause = state->current.primary;
    node->source = state->current.primary_source;
    node->error = state->current.primary_error;
    node->trace = state->current.primary_trace;
    node->next = state->current.head;
    if (incoming.tail)
      incoming.tail->next = node;
    else
      incoming.head = node;
    incoming.tail = state->current.tail ? state->current.tail : node;
    state->current = incoming;
  } else if (incoming.primary) {
    state->current = incoming;
  }
  state->suspended = frame->previous;
  state->depth -= 1;
  free(frame);
}
static inline void rinzu_failure_begin_scoped(rinzu_failure_state_t *state, uint64_t activation,
                                              uint64_t region) {
  rinzu_failure_begin(state);
  state->suspended->activation = activation;
  state->suspended->region = region;
  state->suspended->scoped = 1;
}
static inline int rinzu_failure_end_scoped(rinzu_failure_state_t *state, uint64_t activation,
                                           uint64_t region) {
  for (rinzu_cleanup_frame_t *frame = state->suspended; frame; frame = frame->previous) {
    if (frame->scoped && frame->activation == activation && frame->region == region) {
      if (frame != state->suspended)
        rinzu_failure_fatal();
      rinzu_failure_end(state);
      return 1;
    }
  }
  return 0;
}
static inline int rinzu_failure_is_error(int64_t cause) {
  return cause == 5 || (cause >= 8 && cause <= 15);
}
/* A producer without a managed stack has an explicitly empty snapshot. A later
   await must not invent its own stack as the completed producer's history. */
static inline void rinzu_failure_mark_unrecorded_traces(rinzu_failure_t *failure) {
  if (rinzu_failure_is_error(failure->primary) && !failure->primary_trace)
    failure->primary_trace = rinzu_error_trace_capture(NULL, 0);
  for (rinzu_diagnostic_node_t *node = failure->head; node; node = node->next)
    if (rinzu_failure_is_error(node->cause) && !node->trace)
      node->trace = rinzu_error_trace_capture(NULL, 0);
}
static inline const char *rinzu_failure_name(int64_t cause) {
  switch (cause) {
  case 0:
    return "none";
  case 1:
    return "arithmetic";
  case 2:
    return "cancelled";
  case 3:
    return "overloaded";
  case 4:
    return "shutdown";
  case 5:
    return "conversion";
  case 6:
    return "unsupported";
  case 7:
    return "runtime-panic";
  case 16:
    return "panic";
  case 8:
    return "null-native-value";
  case 9:
    return "invalid-utf8";
  case 10:
    return "native-record-busy";
  case 11:
    return "unobserved-child";
  case 12:
    return "io";
  case 13:
    return "supplant-busy";
  case 14:
    return "unbound-value";
  case 15:
    return "user";
  default:
    rinzu_failure_fatal();
    return NULL;
  }
}
static inline int rinzu_failure_recoverable(const rinzu_failure_state_t *state) {
  if (!rinzu_failure_is_error(state->current.primary))
    return 0;
  for (rinzu_diagnostic_node_t *node = state->current.head; node; node = node->next) {
    if (!rinzu_failure_is_error(node->cause))
      return 0;
  }
  return 1;
}
static inline int64_t rinzu_failure_observe(rinzu_failure_state_t *state) {
  if (!rinzu_failure_recoverable(state) || state->observed_count == INT64_MAX)
    rinzu_failure_fatal();
  rinzu_observed_error_t *error = rinzu_failure_allocate(sizeof(*error));
  error->identity = ++state->observed_count;
  error->failure = state->current;
  error->next = state->observed;
  state->observed = error;
  state->current = (rinzu_failure_t){0};
  return (int64_t)error->identity;
}
/* Release exactly the diagnostic chain owned by the caller. Thin managed
   Errors retain their primary trace separately. */
static __attribute__((noinline, cold)) void
rinzu_diagnostic_nodes_release(rinzu_diagnostic_node_t *head) {
  rinzu_diagnostic_node_t *node = head;
  while (node) {
    rinzu_diagnostic_node_t *next = node->next;
    rinzu_error_reference_release(node->error);
    rinzu_error_trace_release(node->trace);
    free(node);
    node = next;
  }
}
static inline void rinzu_failure_release(rinzu_failure_t failure) {
  rinzu_error_reference_release(failure.primary_error);
  rinzu_error_trace_release(failure.primary_trace);
  if (failure.head)
    rinzu_diagnostic_nodes_release(failure.head);
}
static inline rinzu_failure_t rinzu_failure_copy(const rinzu_failure_t *failure) {
  rinzu_failure_t copy = {0};
  copy.primary = failure->primary;
  copy.primary_source = failure->primary_source;
  copy.primary_error = rinzu_error_reference_copy(failure->primary_error);
  copy.primary_trace = rinzu_error_trace_copy(failure->primary_trace);
  for (rinzu_diagnostic_node_t *node = failure->head; node; node = node->next) {
    rinzu_diagnostic_node_t *item = rinzu_failure_allocate(sizeof(*item));
    item->cause = node->cause;
    item->source = node->source;
    item->error = rinzu_error_reference_copy(node->error);
    item->trace = rinzu_error_trace_copy(node->trace);
    if (copy.tail)
      copy.tail->next = item;
    else
      copy.head = item;
    copy.tail = item;
  }
  return copy;
}
static inline void rinzu_failure_forget_managed(rinzu_failure_t *failure) {
  rinzu_error_reference_release(failure->primary_error);
  failure->primary_error = NULL;
  for (rinzu_diagnostic_node_t *node = failure->head; node; node = node->next) {
    rinzu_error_reference_release(node->error);
    node->error = NULL;
  }
}
static inline void rinzu_failure_print_diagnostic(int64_t cause, rinzu_source_position_t source) {
  printf("%lld", (long long)cause);
  if (!cause)
    return;
  if (source.present)
    printf("@%llu:%u:%u", (unsigned long long)source.file, source.start_byte, source.end_byte);
  else
    printf("@-");
}
static inline void rinzu_failure_print_one(const rinzu_failure_t *failure) {
  rinzu_failure_print_diagnostic(failure->primary, failure->primary_source);
  for (rinzu_diagnostic_node_t *node = failure->head; node; node = node->next) {
    printf(",");
    rinzu_failure_print_diagnostic(node->cause, node->source);
  }
}
/* Observation records are prepended with consecutive identities. Build a
   report-local order once; never restart the linked-list search per identity.
   The borrowed records and their trace/payload references stay untouched. */
static inline const rinzu_observed_error_t **
rinzu_failure_observed_order(const rinzu_failure_state_t *state) {
  if (state->observed_count > SIZE_MAX / sizeof(const rinzu_observed_error_t *))
    rinzu_failure_fatal();
  size_t count = (size_t)state->observed_count;
  const rinzu_observed_error_t **ordered =
      count ? rinzu_failure_allocate(count * sizeof(*ordered)) : NULL;
  size_t remaining = count;
  for (const rinzu_observed_error_t *error = state->observed; error; error = error->next) {
    if (!remaining || error->identity != remaining)
      rinzu_failure_fatal();
    ordered[--remaining] = error;
  }
  if (remaining)
    rinzu_failure_fatal();
  return ordered;
}
static inline void rinzu_failure_print(const rinzu_failure_state_t *state) {
  const rinzu_observed_error_t **ordered = rinzu_failure_observed_order(state);
  printf("RINZU_DIAGNOSTICS=");
  rinzu_failure_print_one(&state->current);
  printf("\nRINZU_CAUGHT_ERRORS=");
  for (size_t index = 0; index < (size_t)state->observed_count; ++index) {
    if (index)
      printf(";");
    rinzu_failure_print_one(&ordered[index]->failure);
  }
  printf("\n");
  free(ordered);
}
static inline void rinzu_failure_dispose(rinzu_failure_state_t *state) {
  if (state->suspended || state->depth)
    rinzu_failure_fatal();
  rinzu_failure_release(state->current);
  rinzu_observed_error_t *error = state->observed;
  while (error) {
    rinzu_observed_error_t *next = error->next;
    rinzu_failure_release(error->failure);
    free(error);
    error = next;
  }
  *state = (rinzu_failure_state_t){0};
}
#endif
