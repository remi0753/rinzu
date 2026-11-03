#ifndef RINZU_SUPPLANT_H
#define RINZU_SUPPLANT_H
#include "rinzu/thread_identity.h"
#include <stdint.h>
#include <unistd.h>

/* One process Main start slot. No lock spans native code or origin cleanup. */
enum {
  RINZU_SUPPLANT_IDLE,
  RINZU_SUPPLANT_PREPARING,
  RINZU_SUPPLANT_QUEUED,
  RINZU_SUPPLANT_ENTERING,
  RINZU_SUPPLANT_RUNNING,
  RINZU_SUPPLANT_CANCEL_REQUESTED,
  RINZU_SUPPLANT_SETTLING
};
enum {
  RINZU_SUPPLANT_NO_WINNER,
  RINZU_SUPPLANT_RETURNED,
  RINZU_SUPPLANT_FAILED,
  RINZU_SUPPLANT_CANCELLED,
  RINZU_SUPPLANT_SHUTDOWN
};
typedef struct {
  uint64_t generation;
  unsigned phase, winner, entered;
} rinzu_supplant_snapshot_t;
typedef struct {
  pthread_mutex_t gate;
  pthread_t owner;
  uint64_t next_generation;
  rinzu_supplant_snapshot_t current;
} rinzu_supplant_t;
static inline void rinzu_supplant_lock(rinzu_supplant_t *state) {
  if (!state || pthread_mutex_lock(&state->gate))
    _Exit(86);
}
static inline void rinzu_supplant_unlock(rinzu_supplant_t *state) {
  if (pthread_mutex_unlock(&state->gate))
    _Exit(86);
}
static inline void rinzu_supplant_owner(rinzu_supplant_t *state) {
  if (!state || !rinzu_thread_equal(state->owner, pthread_self()))
    _Exit(86);
}
static inline void rinzu_supplant_init(rinzu_supplant_t *state) {
  if (!state)
    _Exit(86);
  *state = (rinzu_supplant_t){0};
  state->owner = pthread_self();
  if (pthread_mutex_init(&state->gate, NULL))
    _Exit(71);
}
static inline rinzu_supplant_snapshot_t *rinzu_supplant_active(rinzu_supplant_t *state,
                                                               uint64_t generation) {
  if (!generation || state->current.generation != generation)
    _Exit(86);
  return &state->current;
}
/* Busy is the recoverable SupplantBusy Error; it never changes the old slot. */
static inline int rinzu_supplant_reserve(rinzu_supplant_t *state, uint64_t *generation) {
  rinzu_supplant_owner(state);
  if (!generation)
    _Exit(86);
  rinzu_supplant_lock(state);
  if (state->current.phase != RINZU_SUPPLANT_IDLE) {
    rinzu_supplant_unlock(state);
    return 1;
  }
  if (state->next_generation == UINT64_MAX)
    _Exit(86);
  *generation = ++state->next_generation;
  state->current = (rinzu_supplant_snapshot_t){*generation, RINZU_SUPPLANT_PREPARING, 0, 0};
  rinzu_supplant_unlock(state);
  return 0;
}
static inline int rinzu_supplant_queue(rinzu_supplant_t *state, uint64_t generation) {
  rinzu_supplant_owner(state);
  rinzu_supplant_lock(state);
  rinzu_supplant_snapshot_t *active = rinzu_supplant_active(state, generation);
  if (active->phase == RINZU_SUPPLANT_SETTLING && !active->entered) {
    rinzu_supplant_unlock(state);
    return 0;
  }
  if (active->phase != RINZU_SUPPLANT_PREPARING)
    _Exit(86);
  active->phase = RINZU_SUPPLANT_QUEUED;
  rinzu_supplant_unlock(state);
  return 1;
}
static inline int rinzu_supplant_begin_entry(rinzu_supplant_t *state, uint64_t generation) {
  rinzu_supplant_owner(state);
  rinzu_supplant_lock(state);
  rinzu_supplant_snapshot_t *active = rinzu_supplant_active(state, generation);
  if (active->phase == RINZU_SUPPLANT_SETTLING && !active->entered) {
    rinzu_supplant_unlock(state);
    return 0;
  }
  if (active->phase != RINZU_SUPPLANT_QUEUED)
    _Exit(86);
  active->phase = RINZU_SUPPLANT_ENTERING;
  rinzu_supplant_unlock(state);
  return 1;
}
static inline void rinzu_supplant_entered(rinzu_supplant_t *state, uint64_t generation) {
  rinzu_supplant_owner(state);
  rinzu_supplant_lock(state);
  rinzu_supplant_snapshot_t *active = rinzu_supplant_active(state, generation);
  if (active->entered || (active->phase != RINZU_SUPPLANT_ENTERING &&
                          active->phase != RINZU_SUPPLANT_CANCEL_REQUESTED))
    _Exit(86);
  active->entered = 1;
  if (active->phase == RINZU_SUPPLANT_ENTERING)
    active->phase = RINZU_SUPPLANT_RUNNING;
  rinzu_supplant_unlock(state);
}
/* Cancellation notifiers operate on nonmoving metadata only; old generations
   and later losers are ignored without touching roots or a replacement call. */
static inline int rinzu_supplant_cancel(rinzu_supplant_t *state, uint64_t generation,
                                        unsigned winner) {
  if (winner != RINZU_SUPPLANT_CANCELLED && winner != RINZU_SUPPLANT_SHUTDOWN)
    _Exit(86);
  rinzu_supplant_lock(state);
  rinzu_supplant_snapshot_t *active = &state->current;
  if (!generation || active->generation != generation || active->winner) {
    rinzu_supplant_unlock(state);
    return 0;
  }
  active->winner = winner;
  if (active->phase == RINZU_SUPPLANT_PREPARING || active->phase == RINZU_SUPPLANT_QUEUED)
    active->phase = RINZU_SUPPLANT_SETTLING;
  else if (active->phase == RINZU_SUPPLANT_ENTERING || active->phase == RINZU_SUPPLANT_RUNNING)
    active->phase = RINZU_SUPPLANT_CANCEL_REQUESTED;
  else
    _Exit(86);
  rinzu_supplant_unlock(state);
  return 1;
}
static inline int rinzu_supplant_fail_preparation(rinzu_supplant_t *state, uint64_t generation) {
  rinzu_supplant_owner(state);
  rinzu_supplant_lock(state);
  rinzu_supplant_snapshot_t *active = rinzu_supplant_active(state, generation);
  if (active->winner) {
    rinzu_supplant_unlock(state);
    return 0;
  }
  if (active->phase != RINZU_SUPPLANT_PREPARING && active->phase != RINZU_SUPPLANT_QUEUED)
    _Exit(86);
  active->winner = RINZU_SUPPLANT_FAILED;
  active->phase = RINZU_SUPPLANT_SETTLING;
  rinzu_supplant_unlock(state);
  return 1;
}
static inline void rinzu_supplant_returned(rinzu_supplant_t *state, uint64_t generation) {
  rinzu_supplant_owner(state);
  rinzu_supplant_lock(state);
  rinzu_supplant_snapshot_t *active = rinzu_supplant_active(state, generation);
  if (!active->entered ||
      (active->phase != RINZU_SUPPLANT_RUNNING && active->phase != RINZU_SUPPLANT_CANCEL_REQUESTED))
    _Exit(86);
  if (!active->winner)
    active->winner = RINZU_SUPPLANT_RETURNED;
  active->phase = RINZU_SUPPLANT_SETTLING;
  rinzu_supplant_unlock(state);
}
/* Call only after callback/root/lease settlement; result roots stay owned by
   their continuation. Physical return alone never releases the start slot. */
static inline unsigned rinzu_supplant_finish_settlement(rinzu_supplant_t *state,
                                                        uint64_t generation) {
  rinzu_supplant_owner(state);
  rinzu_supplant_lock(state);
  rinzu_supplant_snapshot_t *active = rinzu_supplant_active(state, generation);
  if (active->phase != RINZU_SUPPLANT_SETTLING || !active->winner)
    _Exit(86);
  unsigned winner = active->winner;
  *active = (rinzu_supplant_snapshot_t){0};
  rinzu_supplant_unlock(state);
  return winner;
}
static inline rinzu_supplant_snapshot_t rinzu_supplant_snapshot(rinzu_supplant_t *state) {
  rinzu_supplant_lock(state);
  rinzu_supplant_snapshot_t snapshot = state->current;
  rinzu_supplant_unlock(state);
  return snapshot;
}
static inline void rinzu_supplant_dispose(rinzu_supplant_t *state) {
  rinzu_supplant_owner(state);
  if (rinzu_supplant_snapshot(state).phase != RINZU_SUPPLANT_IDLE)
    _Exit(86);
  if (pthread_mutex_destroy(&state->gate))
    _Exit(86);
}
#endif
