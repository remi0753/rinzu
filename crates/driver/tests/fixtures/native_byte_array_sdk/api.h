#ifndef SPAN_RESULT_FIXTURE_H
#define SPAN_RESULT_FIXTURE_H
#include <stddef.h>
#include <stdint.h>
/* Deliberately use a different C tag: structural ABI checking is required. */
struct SpanResult {
  uint8_t *data;
  size_t length;
};
const struct SpanResult *span_result(int32_t kind);
const struct SpanResult *span_empty(void);
const struct SpanResult *span_callback(size_t (*measure)(const uint8_t *));
void span_mutate(void);
#endif
