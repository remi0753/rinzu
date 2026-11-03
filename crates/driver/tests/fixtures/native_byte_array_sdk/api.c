#include "api.h"
#include <stdlib.h>
#include <string.h>
static _Thread_local uint8_t bytes[65536];
static _Thread_local struct SpanResult result;
const struct SpanResult *span_result(int32_t kind) {
  if (!kind) return NULL;
  result.data = bytes;
  if (kind == 1) {
    const uint8_t small[] = {0, 255, 128, 0};
    memcpy(bytes, small, sizeof(small)); result.length = sizeof(small);
  } else if (kind == 2 || kind == 3) {
    result.data = kind == 2 ? NULL : (uint8_t *)UINTPTR_MAX; result.length = 0;
  } else if (kind == 4) {
    for (size_t i = 0; i < sizeof(bytes); ++i) bytes[i] = (uint8_t)(i * 139);
    result.length = sizeof(bytes);
  } else abort();
  return &result;
}
void span_mutate(void) {
  memset(bytes, 90, sizeof(bytes)); result.data = NULL; result.length = SIZE_MAX;
}
const struct SpanResult *span_empty(void) { return span_result(2); }
const struct SpanResult *span_callback(size_t (*measure)(const uint8_t *)) {
  const uint8_t payload[] = {0, 255, 128, 0};
  if (measure(payload) != sizeof(payload)) abort();
  return span_result(1);
}
