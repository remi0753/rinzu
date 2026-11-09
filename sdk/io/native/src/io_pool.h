#ifndef RINZU_IO_POOL_H
#define RINZU_IO_POOL_H
#include "rinzu/native_completion.h"
#include <errno.h>
#include <stdbool.h>
#include <stdlib.h>

/* A reservation admits one job at a time through the end of cleanup. */
int32_t rinzu_io_reserve(size_t control_bytes);
void rinzu_io_submit_reserved(void (*run)(void *), void *context);
void rinzu_io_publish_reserved(rinzu_native_completion_v2_t *completion);
void rinzu_io_release_reservation(size_t control_bytes);
void rinzu_io_track_resource(bool file, bool acquired);

static inline _Noreturn void fatal(void) {
  _Exit(86);
}
static inline void *allocate(size_t size) {
  void *value = calloc(1, size);
  if (!value)
    _Exit(70);
  return value;
}
static inline char *copy_text(const char *text) {
  size_t length = strlen(text);
  if (length == SIZE_MAX)
    fatal();
  char *copy = allocate(length + 1);
  memcpy(copy, text, length + 1);
  return copy;
}
static inline int32_t native_error(void) {
  if (errno == ENOMEM)
    _Exit(70);
  return errno ? errno : EIO;
}
#endif
