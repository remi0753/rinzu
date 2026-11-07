#ifndef RINZU_ERROR_TRACE_H
#define RINZU_ERROR_TRACE_H
#include <stdint.h>
#include <stdlib.h>
#include <stdatomic.h>
#include <string.h>
#include <unistd.h>
#include "rinzu/native_fatal.h"

typedef struct {
  uint64_t file;
  uint32_t start_byte, end_byte;
  unsigned present;
} rinzu_source_position_t;

/* Diagnostic data only. Names are copied into the immutable snapshot, without
   retaining a frame, local value, managed address, or callable address. */
typedef struct {
  const char *function;
  size_t function_length;
  rinzu_source_position_t source;
} rinzu_error_trace_frame_t;
typedef struct {
  _Atomic size_t references;
  size_t count;
  rinzu_error_trace_frame_t *frames;
} rinzu_error_trace_t;

static inline rinzu_error_trace_t *
rinzu_error_trace_capture(const rinzu_error_trace_frame_t *frames, size_t count) {
  if ((count && !frames) || count > (SIZE_MAX - sizeof(rinzu_error_trace_t)) / sizeof(*frames))
    _Exit(70);
  size_t size = sizeof(rinzu_error_trace_t) + count * sizeof(*frames);
  for (size_t i = 0; i < count; ++i) {
    if ((frames[i].function_length && !frames[i].function) ||
        frames[i].function_length == SIZE_MAX || frames[i].function_length + 1 > SIZE_MAX - size)
      _Exit(70);
    size += frames[i].function_length + 1;
  }
  rinzu_error_trace_t *trace = calloc(1, size);
  if (!trace)
    rinzu_resource_fatal();
  atomic_init(&trace->references, 1);
  trace->count = count;
  trace->frames = (rinzu_error_trace_frame_t *)(trace + 1);
  char *names = (char *)(trace->frames + count);
  for (size_t i = 0; i < count; ++i) {
    trace->frames[i] = frames[i];
    trace->frames[i].function = names;
    if (frames[i].function_length)
      memcpy(names, frames[i].function, frames[i].function_length);
    names += frames[i].function_length + 1;
  }
  return trace;
}
static inline rinzu_error_trace_t *rinzu_error_trace_copy(rinzu_error_trace_t *trace) {
  if (trace) {
    size_t before = atomic_fetch_add_explicit(&trace->references, 1, memory_order_relaxed);
    if (!before || before == SIZE_MAX)
      _Exit(70);
  }
  return trace;
}
static inline void rinzu_error_trace_release(rinzu_error_trace_t *trace) {
  if (trace) {
    size_t before = atomic_fetch_sub_explicit(&trace->references, 1, memory_order_acq_rel);
    if (!before)
      _Exit(70);
    if (before == 1)
      free(trace);
  }
}
typedef struct {
  rinzu_error_trace_frame_t *frames;
  size_t count, capacity;
  int initialized, enabled;
} rinzu_error_trace_stack_t;
#ifndef RINZU_ERROR_TRACE_DEFAULT
#define RINZU_ERROR_TRACE_DEFAULT 0
#endif
static inline int rinzu_error_trace_enabled(void) {
  const char *enabled = getenv("RINZU_ERROR_TRACE");
  return RINZU_ERROR_TRACE_DEFAULT || (enabled && !strcmp(enabled, "1"));
}
static inline void rinzu_error_trace_stack_init(rinzu_error_trace_stack_t *stack) {
  if (!stack->initialized) {
    stack->enabled = rinzu_error_trace_enabled();
    stack->initialized = 1;
  }
}
static inline void rinzu_error_trace_stack_source(rinzu_error_trace_stack_t *stack,
                                                  rinzu_source_position_t source) {
  if (stack && stack->enabled && stack->count)
    stack->frames[stack->count - 1].source = source;
}
static inline void rinzu_error_trace_stack_enter(rinzu_error_trace_stack_t *stack, const char *name,
                                                 size_t length) {
  rinzu_error_trace_stack_init(stack);
  if (!stack->enabled)
    return;
  if (stack->count == stack->capacity) {
    if (stack->capacity > SIZE_MAX / 2 / sizeof(*stack->frames))
      _Exit(70);
    size_t capacity = stack->capacity ? stack->capacity * 2 : 8;
    void *frames = realloc(stack->frames, capacity * sizeof(*stack->frames));
    if (!frames)
      rinzu_resource_fatal();
    stack->frames = frames;
    stack->capacity = capacity;
  }
  stack->frames[stack->count++] = (rinzu_error_trace_frame_t){name, length, {0}};
}
static inline void rinzu_error_trace_stack_leave(rinzu_error_trace_stack_t *stack) {
  if (stack->enabled) {
    if (!stack->count)
      _Exit(70);
    stack->count--;
    if (!stack->count) {
      free(stack->frames);
      stack->frames = NULL;
      stack->capacity = 0;
    }
  }
}
static inline rinzu_error_trace_t *
rinzu_error_trace_stack_capture(const rinzu_error_trace_stack_t *stack,
                                rinzu_source_position_t source) {
  if (!stack || !stack->enabled)
    return NULL;
  size_t count = stack->count;
  rinzu_error_trace_frame_t *frames = count ? malloc(count * sizeof(*frames)) : NULL;
  if (count && !frames)
    rinzu_resource_fatal();
  for (size_t i = 0; i < count; ++i)
    frames[i] = stack->frames[count - i - 1];
  if (count)
    frames[0].source = source;
  rinzu_error_trace_t *trace = rinzu_error_trace_capture(frames, count);
  free(frames);
  return trace;
}
static inline void rinzu_error_trace_stack_dispose(rinzu_error_trace_stack_t *stack) {
  if (stack->count)
    _Exit(70);
  free(stack->frames);
  *stack = (rinzu_error_trace_stack_t){0};
}
#endif
