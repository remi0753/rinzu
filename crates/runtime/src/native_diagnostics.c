#include "rinzu/native_diagnostics.h"
rinzu_wait_dependency_t *rinzu_diagnostic_wait_begin(rinzu_activity_board_t *board, uint64_t task,
                                                     uint64_t dependency, const char *symbol,
                                                     uint64_t file, uint32_t start, uint32_t end,
                                                     uint64_t owner, uint64_t depth) {
  return rinzu_activity_wait_begin(board, task, dependency, symbol, file, start, end, owner, depth);
}
void rinzu_diagnostic_wait_end(rinzu_wait_dependency_t *wait) {
  rinzu_activity_wait_end(wait);
}
rinzu_activity_board_t *rinzu_diagnostic_board_new(void) {
  return rinzu_activity_new();
}
rinzu_activity_t *rinzu_diagnostic_prepare(rinzu_activity_board_t *board, unsigned kind,
                                           const char *symbol, uint64_t file, uint32_t start_byte,
                                           uint32_t end_byte, uint64_t owner, uint64_t depth) {
  return rinzu_activity_prepare(board, kind, symbol, file, start_byte, end_byte, owner, depth);
}
rinzu_activity_t *rinzu_diagnostic_prepare_native(rinzu_activity_board_t *board, const char *symbol,
                                                  uint64_t file, uint32_t start_byte,
                                                  uint32_t end_byte, uint64_t owner,
                                                  uint64_t depth) {
  return rinzu_activity_prepare_native(board, symbol, file, start_byte, end_byte, owner, depth);
}
void rinzu_diagnostic_start(rinzu_activity_t *activity) {
  rinzu_activity_start(activity);
}
void rinzu_diagnostic_end(rinzu_activity_t *activity) {
  rinzu_activity_end(activity);
}
rinzu_activity_stats_t rinzu_diagnostic_stats(rinzu_activity_board_t *board) {
  return rinzu_activity_stats(board);
}
void rinzu_diagnostic_board_dispose(rinzu_activity_board_t *board) {
  rinzu_activity_dispose(board);
}
void rinzu_diagnostic_owner_turn(rinzu_activity_board_t *board) {
  rinzu_activity_owner_turn(board);
}
void rinzu_diagnostic_request_shutdown(rinzu_activity_board_t *board) {
  rinzu_activity_request_shutdown(board);
}
