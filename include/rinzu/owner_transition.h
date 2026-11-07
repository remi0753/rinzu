#ifndef RINZU_OWNER_TRANSITION_H
#define RINZU_OWNER_TRANSITION_H
#include "rinzu/thread_identity.h"
#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include "rinzu/native_diagnostics.h"
#include "rinzu/ready_work.h"

enum {
  RINZU_OWNER_INACTIVE,
  RINZU_OWNER_MANAGED,
  RINZU_OWNER_NATIVE_SAFE,
  RINZU_OWNER_CALLBACK_ENTRY,
  RINZU_OWNER_CALLBACK_MANAGED
};
typedef struct rinzu_owner_native_frame rinzu_owner_native_frame_t;
typedef struct {
  pthread_t owner;
  unsigned initialized, state;
  uint64_t generation, stack_map;
  size_t roots;
  rinzu_owner_native_frame_t *current;
  rinzu_activity_board_t *activity_board;
} rinzu_owner_transition_t;
struct rinzu_owner_native_frame {
  rinzu_owner_native_frame_t *previous;
  uint64_t generation, previous_stack_map;
  size_t previous_roots;
  unsigned return_state, active;
};
typedef struct {
  rinzu_owner_transition_t *owner;
  uint64_t generation;
  unsigned after_return;
} rinzu_owner_callback_scope_t;
static inline void rinzu_owner_transition_require(rinzu_owner_transition_t *owner) {
  if (!owner || owner->initialized != 1 || !rinzu_thread_equal(owner->owner, pthread_self()))
    _Exit(86);
}
static inline void rinzu_owner_transition_init(rinzu_owner_transition_t *owner) {
  if (!owner)
    _Exit(86);
  if (!owner->initialized) {
    owner->owner = pthread_self();
    owner->initialized = 1;
    owner->state = RINZU_OWNER_MANAGED;
  }
  rinzu_owner_transition_require(owner);
}
static inline void rinzu_owner_transition_managed(rinzu_owner_transition_t *owner) {
  rinzu_owner_transition_init(owner);
  if (owner->state != RINZU_OWNER_MANAGED && owner->state != RINZU_OWNER_CALLBACK_MANAGED)
    _Exit(86);
}
static inline uint64_t rinzu_owner_activity_depth(rinzu_owner_transition_t *owner) {
  rinzu_owner_transition_init(owner);
  if (!owner->activity_board)
    owner->activity_board = rinzu_activity_new();
  uint64_t native_depth = 0;
  for (rinzu_owner_native_frame_t *frame = owner->current; frame; frame = frame->previous)
    if (++native_depth > owner->generation)
      _Exit(86);
  if (native_depth == UINT64_MAX)
    _Exit(86);
  return native_depth + 1;
}
static inline rinzu_activity_t *rinzu_owner_activity_prepare(rinzu_owner_transition_t *owner,
                                                             unsigned kind, const char *symbol,
                                                             uint64_t file, uint32_t start,
                                                             uint32_t end, uint64_t depth) {
  (void)depth;
  /* depth() freshly checks the current thread; no yield/reentry occurs here. */
  uint64_t physical_depth = rinzu_owner_activity_depth(owner);
  if (kind == RINZU_ACTIVITY_NATIVE)
    return rinzu_activity_prepare_native_on(owner->activity_board, symbol, file, start, end,
                                            (uint64_t)(uintptr_t)owner->owner, physical_depth,
                                            owner->owner);
  return rinzu_activity_prepare_on(owner->activity_board, kind, symbol, file, start, end,
                                   (uint64_t)(uintptr_t)owner->owner, physical_depth, owner->owner);
}
static inline rinzu_ready_work_t *rinzu_owner_ready_prepare(rinzu_owner_transition_t *owner,
                                                            const char *symbol, uint64_t file,
                                                            uint32_t start, uint32_t end) {
  uint64_t depth = rinzu_owner_activity_depth(owner);
  return rinzu_ready_prepare_on(owner->activity_board, symbol, file, start, end,
                                (uint64_t)(uintptr_t)owner->owner, depth, owner->owner);
}
static inline rinzu_wait_dependency_t *rinzu_owner_wait_prepare(rinzu_owner_transition_t *owner,
                                                                uint64_t task, uint64_t dependency,
                                                                const char *symbol, uint64_t file,
                                                                uint32_t start, uint32_t end) {
  uint64_t depth = rinzu_owner_activity_depth(owner);
  return rinzu_activity_wait_begin_on(owner->activity_board, task, dependency, symbol, file, start,
                                      end, (uint64_t)(uintptr_t)owner->owner, depth, owner->owner);
}
static inline void rinzu_owner_note_turn(rinzu_owner_transition_t *owner) {
  rinzu_owner_transition_init(owner);
  if (owner->activity_board)
    rinzu_activity_owner_turn(owner->activity_board);
}
static inline void rinzu_owner_request_shutdown(rinzu_owner_transition_t *owner) {
  rinzu_owner_transition_init(owner);
  if (owner->activity_board)
    rinzu_activity_request_shutdown(owner->activity_board);
}
static inline void rinzu_owner_native_enter(rinzu_owner_transition_t *owner,
                                            rinzu_owner_native_frame_t *frame, uint64_t stack_map,
                                            size_t roots) {
  rinzu_owner_transition_managed(owner);
  if (!frame || frame->active || owner->generation == UINT64_MAX)
    _Exit(86);
  *frame = (rinzu_owner_native_frame_t){owner->current, ++owner->generation, owner->stack_map,
                                        owner->roots,   owner->state,        1};
  owner->current = frame;
  owner->stack_map = stack_map;
  owner->roots = roots;
  owner->state = RINZU_OWNER_NATIVE_SAFE;
}
static inline void rinzu_owner_native_leave(rinzu_owner_transition_t *owner,
                                            rinzu_owner_native_frame_t *frame) {
  rinzu_owner_transition_require(owner);
  if (!frame || !frame->active || owner->current != frame ||
      owner->state != RINZU_OWNER_NATIVE_SAFE)
    _Exit(86);
  owner->current = frame->previous;
  owner->stack_map = frame->previous_stack_map;
  owner->roots = frame->previous_roots;
  owner->state = frame->return_state;
  frame->active = 0;
}
static inline rinzu_owner_callback_scope_t
rinzu_owner_callback_enter(rinzu_owner_transition_t *owner) {
  rinzu_owner_transition_require(owner);
  if (owner->state != RINZU_OWNER_NATIVE_SAFE || !owner->current || !owner->current->active)
    _Exit(86);
  rinzu_owner_note_turn(owner);
  owner->state = RINZU_OWNER_CALLBACK_ENTRY;
  rinzu_owner_callback_scope_t scope = {owner, owner->current->generation, 0};
  owner->state = RINZU_OWNER_CALLBACK_MANAGED;
  return scope;
}
static inline void rinzu_owner_callback_leave(rinzu_owner_callback_scope_t scope) {
  rinzu_owner_transition_t *owner = scope.owner;
  rinzu_owner_transition_require(owner);
  if (scope.after_return) {
    if (owner->state != RINZU_OWNER_CALLBACK_MANAGED || owner->current || scope.generation)
      _Exit(86);
    owner->state = RINZU_OWNER_CALLBACK_ENTRY;
    owner->state = RINZU_OWNER_MANAGED;
    return;
  }
  if (owner->state != RINZU_OWNER_CALLBACK_MANAGED || !owner->current ||
      owner->current->generation != scope.generation)
    _Exit(86);
  owner->state = RINZU_OWNER_CALLBACK_ENTRY;
  owner->state = RINZU_OWNER_NATIVE_SAFE;
}
static inline rinzu_owner_callback_scope_t
rinzu_owner_admitted_callback_enter(rinzu_owner_transition_t *owner) {
  rinzu_owner_transition_require(owner);
  if (owner->state == RINZU_OWNER_NATIVE_SAFE)
    return rinzu_owner_callback_enter(owner);
  if (owner->state != RINZU_OWNER_MANAGED || owner->current)
    _Exit(86);
  rinzu_owner_note_turn(owner);
  owner->state = RINZU_OWNER_CALLBACK_ENTRY;
  owner->state = RINZU_OWNER_CALLBACK_MANAGED;
  return (rinzu_owner_callback_scope_t){owner, 0, 1};
}
static inline void rinzu_owner_transition_finish(rinzu_owner_transition_t *owner) {
  if (!owner)
    _Exit(86);
  if (!owner->initialized) {
    owner->initialized = 2;
    return;
  }
  rinzu_owner_transition_require(owner);
  if (owner->state != RINZU_OWNER_MANAGED || owner->current || owner->stack_map || owner->roots)
    _Exit(86);
  if (owner->activity_board) {
    rinzu_activity_dispose(owner->activity_board);
    owner->activity_board = NULL;
  }
  owner->state = RINZU_OWNER_INACTIVE;
  owner->initialized = 2;
}
#endif
