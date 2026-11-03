#include "rinzu/foreign_queue.h"
rinzu_foreign_queue_t *rinzu_foreign_queue_create(rinzu_foreign_notify_t wake, void *context) {
  return rinzu_foreign_new(wake, context);
}
void rinzu_foreign_queue_drop(rinzu_foreign_queue_t *queue) {
  rinzu_foreign_dispose(queue);
}
rinzu_foreign_stats_t rinzu_foreign_queue_snapshot(rinzu_foreign_queue_t *queue) {
  return rinzu_foreign_stats(queue);
}
void rinzu_foreign_queue_wake(rinzu_foreign_queue_t *queue) {
  rinzu_foreign_wake(queue);
}
void rinzu_foreign_queue_progress(rinzu_foreign_queue_t *queue, unsigned operation,
                                  uint64_t generation) {
  switch (operation) {
  case 0:
    rinzu_foreign_attach_main(queue);
    return;
  case 1:
    rinzu_foreign_loop_begin(queue, generation);
    return;
  case 2:
    rinzu_foreign_loop_end(queue, generation);
    return;
  case 3:
    rinzu_foreign_bounded_enter(queue);
    return;
  case 4:
    rinzu_foreign_bounded_leave(queue);
    return;
  default:
    _Exit(86);
  }
}
rinzu_foreign_record_t *rinzu_foreign_queue_take(rinzu_foreign_queue_t *queue) {
  return rinzu_foreign_pop(queue);
}
uint64_t rinzu_foreign_queue_invoke(rinzu_foreign_queue_t *queue, uint64_t callback,
                                    uint64_t generation, const uint64_t *words, size_t count,
                                    rinzu_foreign_notify_t receipt, void *context) {
  if (!queue || rinzu_thread_equal(queue->owner, pthread_self()))
    _Exit(86);
  rinzu_foreign_record_t *record =
      rinzu_foreign_record_new(queue, callback, generation, words, count, receipt, context);
  rinzu_foreign_publish(record);
  uint64_t result = rinzu_foreign_receive(record);
  rinzu_foreign_release(record);
  return result;
}
void rinzu_foreign_queue_complete(rinzu_foreign_record_t *record, uint64_t result) {
  rinzu_foreign_complete(record, result);
  rinzu_foreign_release(record);
}
const uint64_t *rinzu_foreign_queue_words(rinzu_foreign_record_t *record, size_t *count) {
  rinzu_foreign_owner(record->queue);
  if (record->state != RINZU_FOREIGN_EXECUTING)
    _Exit(86);
  *count = record->count;
  return record->words;
}
uint64_t rinzu_foreign_queue_identity(rinzu_foreign_record_t *record, unsigned field) {
  rinzu_foreign_owner(record->queue);
  switch (field) {
  case 0:
    return record->identity;
  case 1:
    return record->sequence;
  case 2:
    return record->callback;
  case 3:
    return record->generation;
  default:
    _Exit(86);
  }
}
