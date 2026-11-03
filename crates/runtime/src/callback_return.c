#include "rinzu/callback_consumption.h"
size_t rinzu_callback_return_node_size(void) {
  return sizeof(rinzu_callback_return_node_t);
}
const unsigned char *rinzu_callback_return_copy(const unsigned char *input, size_t prefix,
                                                size_t length, unsigned string, unsigned fallback) {
  return rinzu_callback_return_buffer_copy(input, prefix, length, string, fallback);
}
rinzu_callback_return_stats_t rinzu_callback_return_stats(void) {
  return rinzu_callback_return_buffer_stats();
}
size_t rinzu_callback_return_copy_bytes(void) {
  return rinzu_callback_return_process_copy_bytes();
}
uint64_t rinzu_callback_scope_prepare(void) {
  return rinzu_return_scope_prepare();
}
unsigned rinzu_callback_scope_enter(uint64_t id) {
  return rinzu_return_scope_enter(id);
}
unsigned rinzu_callback_scope_finish(uint64_t id, unsigned mode) {
  return rinzu_return_scope_finish(id, mode);
}
const unsigned char *rinzu_callback_scope_copy(uint64_t id, const unsigned char *input,
                                               size_t prefix, size_t length, unsigned string,
                                               unsigned fallback) {
  return rinzu_return_scope_copy(id, input, prefix, length, string, fallback);
}
rinzu_return_scope_stats_t rinzu_callback_scope_stats(void) {
  return rinzu_return_scope_stats();
}
rinzu_return_snapshot_t rinzu_callback_return_snapshot(void) {
  return rinzu_return_snapshot();
}
