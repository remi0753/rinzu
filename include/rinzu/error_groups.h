#ifndef RINZU_ERROR_GROUPS_H
#define RINZU_ERROR_GROUPS_H
#include "rinzu/failure.h"

/* Lexical Error collection is independent of task admission and completion. */
typedef struct rinzu_group_failure {
  rinzu_failure_t failure;
  struct rinzu_group_failure *next;
} rinzu_group_failure_t;
typedef struct {
  uint64_t identity;
  unsigned present;
} rinzu_group_origin_t;
typedef struct rinzu_error_group {
  uint64_t scope;
  rinzu_group_failure_t *head, *tail;
  rinzu_group_origin_t *origins;
  size_t count, capacity;
  struct rinzu_error_group *previous;
} rinzu_error_group_t;
typedef struct {
  rinzu_error_group_t *top;
  size_t depth;
} rinzu_error_groups_t;

static inline size_t rinzu_group_origin_index(uint64_t identity, size_t capacity) {
  identity ^= identity >> 30;
  identity *= UINT64_C(0xbf58476d1ce4e5b9);
  identity ^= identity >> 27;
  identity *= UINT64_C(0x94d049bb133111eb);
  identity ^= identity >> 31;
  return (size_t)identity & (capacity - 1);
}
static inline size_t rinzu_group_origin_slot(rinzu_group_origin_t *slots, size_t capacity,
                                             uint64_t identity) {
  size_t slot = rinzu_group_origin_index(identity, capacity);
  while (slots[slot].present && slots[slot].identity != identity)
    slot = (slot + 1) & (capacity - 1);
  return slot;
}
/* Returns zero for an already observed Future. No origin is required for an
   explicit throw, so repeated throws of the same Error are not deduplicated. */
static inline int rinzu_group_observe(rinzu_error_group_t *group, uint64_t identity) {
  if (group->capacity) {
    size_t slot = rinzu_group_origin_slot(group->origins, group->capacity, identity);
    if (group->origins[slot].present)
      return 0;
  }
  if (!group->capacity || group->count >= group->capacity / 2) {
    if (group->capacity > SIZE_MAX / 2 / sizeof(*group->origins))
      rinzu_failure_fatal();
    size_t capacity = group->capacity ? group->capacity * 2 : 16;
    rinzu_group_origin_t *slots = rinzu_failure_allocate(capacity * sizeof(*slots));
    for (size_t index = 0; index < group->capacity; index++)
      if (group->origins[index].present) {
        size_t slot = rinzu_group_origin_slot(slots, capacity, group->origins[index].identity);
        slots[slot] = group->origins[index];
      }
    free(group->origins);
    group->origins = slots;
    group->capacity = capacity;
  }
  size_t slot = rinzu_group_origin_slot(group->origins, group->capacity, identity);
  group->origins[slot] = (rinzu_group_origin_t){identity, 1};
  group->count++;
  return 1;
}
static inline rinzu_error_group_t *rinzu_error_group_current(rinzu_error_groups_t *groups,
                                                             uint64_t scope) {
  if (!groups->top || groups->top->scope != scope)
    rinzu_failure_fatal();
  return groups->top;
}
static inline void rinzu_error_group_enter(rinzu_error_groups_t *groups, uint64_t scope) {
  if (groups->depth == SIZE_MAX)
    rinzu_failure_fatal();
  rinzu_error_group_t *group = rinzu_failure_allocate(sizeof(*group));
  group->scope = scope;
  group->previous = groups->top;
  groups->top = group;
  groups->depth++;
}
static inline int rinzu_error_group_record(rinzu_error_groups_t *groups, uint64_t scope,
                                           unsigned has_origin, uint64_t origin,
                                           rinzu_failure_t *failure) {
  rinzu_error_group_t *group = rinzu_error_group_current(groups, scope);
  if (!failure || !rinzu_failure_is_error(failure->primary))
    rinzu_failure_fatal();
  for (rinzu_diagnostic_node_t *node = failure->head; node; node = node->next)
    if (!rinzu_failure_is_error(node->cause))
      rinzu_failure_fatal();
  if (has_origin && !rinzu_group_observe(group, origin)) {
    rinzu_failure_release(*failure);
    *failure = (rinzu_failure_t){0};
    return 0;
  }
  rinzu_group_failure_t *entry = rinzu_failure_allocate(sizeof(*entry));
  entry->failure = *failure;
  *failure = (rinzu_failure_t){0};
  if (group->tail)
    group->tail->next = entry;
  else
    group->head = entry;
  group->tail = entry;
  return 1;
}
static inline rinzu_group_failure_t *rinzu_error_group_finish(rinzu_error_groups_t *groups,
                                                              uint64_t scope) {
  rinzu_error_group_t *group = rinzu_error_group_current(groups, scope);
  if (!groups->depth)
    rinzu_failure_fatal();
  rinzu_group_failure_t *entries = group->head;
  groups->top = group->previous;
  groups->depth--;
  free(group->origins);
  free(group);
  return entries;
}
/* Move an ordered failure without cloning its payload or diagnostic nodes. */
static inline void rinzu_error_group_append(rinzu_failure_t *target, rinzu_failure_t failure) {
  if (!failure.primary)
    return;
  if (!target->primary) {
    *target = failure;
    return;
  }
  rinzu_diagnostic_node_t *entry = rinzu_failure_allocate(sizeof(*entry));
  entry->cause = failure.primary;
  entry->source = failure.primary_source;
  entry->error = failure.primary_error;
  entry->trace = failure.primary_trace;
  entry->next = failure.head;
  if (target->tail)
    target->tail->next = entry;
  else
    target->head = entry;
  target->tail = failure.tail ? failure.tail : entry;
}
static inline rinzu_failure_t rinzu_error_group_aggregate(rinzu_group_failure_t *entries) {
  rinzu_failure_t result = {0};
  while (entries) {
    rinzu_group_failure_t *next = entries->next;
    rinzu_error_group_append(&result, entries->failure);
    free(entries);
    entries = next;
  }
  return result;
}
static inline void rinzu_error_groups_dispose(rinzu_error_groups_t *groups) {
  while (groups->top) {
    rinzu_failure_t failure =
        rinzu_error_group_aggregate(rinzu_error_group_finish(groups, groups->top->scope));
    rinzu_failure_release(failure);
  }
}
#endif
