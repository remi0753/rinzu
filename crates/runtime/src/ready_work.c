#include "rinzu/ready_work.h"
rinzu_ready_work_t *rinzu_ready_work_prepare(rinzu_activity_board_t *board, const char *symbol,
                                             uint64_t file, uint32_t start, uint32_t end,
                                             uint64_t owner, uint64_t depth) {
  return rinzu_ready_prepare(board, symbol, file, start, end, owner, depth);
}
void rinzu_ready_work_publish(rinzu_ready_work_t *work) {
  rinzu_ready_publish(work);
}
void rinzu_ready_work_receive(rinzu_ready_work_t *work) {
  rinzu_ready_receive(work);
}
void rinzu_ready_work_dispose(rinzu_ready_work_t *work) {
  rinzu_ready_dispose(work);
}
