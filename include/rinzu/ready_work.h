#ifndef RINZU_READY_WORK_H
#define RINZU_READY_WORK_H
#include "rinzu/thread_identity.h"
#include "rinzu/native_diagnostics.h"

/* Physical completion publication and Owner receipt are separate, ordered by
   this native gate. Receipt can win before the producer's notification; that
   later publication must not invent pending work or restart a settled timer.
   The caller pins this record through both sides, independently of any heap. */
typedef struct {
  pthread_mutex_t gate;
  pthread_t owner;
  rinzu_activity_board_t *board;
  rinzu_activity_t *activity;
  unsigned published, received;
} rinzu_ready_work_t;
static inline rinzu_ready_work_t *rinzu_ready_prepare_on(rinzu_activity_board_t *board,
                                                         const char *symbol, uint64_t file,
                                                         uint32_t start, uint32_t end,
                                                         uint64_t owner, uint64_t depth,
                                                         pthread_t origin) {
  if (!board || !rinzu_thread_equal(board->owner, origin))
    _Exit(86);
  rinzu_ready_work_t *work = rinzu_foreign_allocate(sizeof(*work));
  rinzu_foreign_sync(pthread_mutex_init(&work->gate, NULL));
  work->owner = origin;
  work->board = board;
  work->activity = rinzu_activity_prepare_on(board, RINZU_ACTIVITY_OWNER_STALL, symbol, file, start,
                                             end, owner, depth, origin);
  rinzu_activity_gate_lock(board);
  if (board->stats.ready_live == SIZE_MAX || sizeof(*work) > SIZE_MAX - board->stats.ready_bytes)
    _Exit(71);
  board->stats.ready_live++;
  board->stats.ready_bytes += sizeof(*work);
  rinzu_activity_gate_unlock(board);
  return work;
}
static inline rinzu_ready_work_t *rinzu_ready_prepare(rinzu_activity_board_t *board,
                                                      const char *symbol, uint64_t file,
                                                      uint32_t start, uint32_t end, uint64_t owner,
                                                      uint64_t depth) {
  return rinzu_ready_prepare_on(board, symbol, file, start, end, owner, depth, pthread_self());
}
static inline void rinzu_ready_publish(rinzu_ready_work_t *work) {
  if (!work)
    _Exit(86);
  rinzu_foreign_lock(&work->gate);
  if (work->published)
    _Exit(86);
  work->published = 1;
  if (!work->received) {
    if (!work->activity)
      _Exit(86);
    rinzu_activity_publish_ready(work->activity);
  }
  rinzu_foreign_unlock(&work->gate);
}
static inline void rinzu_ready_receive(rinzu_ready_work_t *work) {
  if (!work || !rinzu_thread_equal(work->owner, pthread_self()))
    _Exit(86);
  rinzu_foreign_lock(&work->gate);
  if (work->received || !work->activity)
    _Exit(86);
  work->received = 1;
  rinzu_activity_end(work->activity);
  work->activity = NULL;
  rinzu_foreign_unlock(&work->gate);
}
static inline void rinzu_ready_dispose(rinzu_ready_work_t *work) {
  if (!work)
    _Exit(86);
  rinzu_foreign_lock(&work->gate);
  if (!work->received || work->activity)
    _Exit(86);
  rinzu_activity_board_t *board = work->board;
  rinzu_activity_gate_lock(board);
  if (!board->stats.ready_live || board->stats.ready_bytes < sizeof(*work))
    _Exit(86);
  board->stats.ready_live--;
  board->stats.ready_bytes -= sizeof(*work);
  rinzu_activity_gate_unlock(board);
  rinzu_foreign_unlock(&work->gate);
  rinzu_foreign_sync(pthread_mutex_destroy(&work->gate));
  free(work);
}
#endif
