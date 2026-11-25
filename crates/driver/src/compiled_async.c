#ifndef RINZU_PROGRAM_API
#define RINZU_PROGRAM_API static
#endif
#include <dlfcn.h>
#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <stdio.h>
#include <stdarg.h>
#include <string.h>
#include <unistd.h>
#include <time.h>
/* RINZU_JIT_SHARED_BEGIN */
typedef struct {
  uint64_t native_startup, native_dlopen, native_images, main;
} rinzu_jit_timing_t;
/* RINZU_JIT_SHARED_END */
static rinzu_jit_timing_t *rinzu_jit_timing;
RINZU_PROGRAM_API uint64_t rinzu_jit_clock(void) {
  struct timespec now;
  if (clock_gettime(CLOCK_MONOTONIC, &now))
    return 0;
  return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}
static __attribute__((unused)) void *rinzu_jit_dlopen(const char *path, int flags) {
  if (!rinzu_jit_timing)
    return dlopen(path, flags);
  uint64_t start = rinzu_jit_clock();
  void *handle = dlopen(path, flags);
  int error = errno;
  uint64_t end = rinzu_jit_clock();
  if (start && end >= start)
    rinzu_jit_timing->native_dlopen += end - start;
  ++rinzu_jit_timing->native_images;
  errno = error;
  return handle;
}
#define RINZU_SOURCE_DLOPEN rinzu_jit_dlopen
/* JIT_ENTRY_COUNTS */
static uintptr_t rinzu_jit_functions[RINZU_JIT_FUNCTION_COUNT ? RINZU_JIT_FUNCTION_COUNT : 1];
static uintptr_t rinzu_jit_stubs[RINZU_JIT_STUB_COUNT ? RINZU_JIT_STUB_COUNT : 1];
static uint64_t rinzu_jit_cancel_after, rinzu_jit_shutdown_after, rinzu_jit_cancel_worker_after;
static const char *rinzu_jit_directory;
static char *rinzu_jit_report;
static size_t rinzu_jit_report_size, rinzu_jit_report_capacity;
static int rinzu_jit_used;
static uint64_t (*rinzu_jit_context_identity)(void);
static void (*rinzu_jit_ready)(void *);
static void *rinzu_jit_ready_context;
RINZU_PROGRAM_API void rinzu_jit_publish(void) {
  if (!rinzu_jit_ready)
    _Exit(86);
  void (*ready)(void *) = rinzu_jit_ready;
  rinzu_jit_ready = NULL;
  ready(rinzu_jit_ready_context);
  rinzu_jit_ready_context = NULL;
}
#define RINZU_CONTEXT_IDENTITY()                                                                   \
  (rinzu_jit_context_identity ? rinzu_jit_context_identity() : (_Exit(86), UINT64_C(0)))
RINZU_PROGRAM_API int rinzu_jit_printf(const char *format, ...) {
  va_list arguments, copy;
  va_start(arguments, format);
  va_copy(copy, arguments);
  int length = vsnprintf(NULL, 0, format, copy);
  va_end(copy);
  if (length < 0)
    _Exit(86);
  size_t count = (size_t)length;
  if (count == SIZE_MAX || rinzu_jit_report_size > SIZE_MAX - count - 1)
    _Exit(71);
  size_t needed = rinzu_jit_report_size + count + 1;
  if (needed > rinzu_jit_report_capacity) {
    size_t capacity = rinzu_jit_report_capacity ? rinzu_jit_report_capacity : 256;
    while (capacity < needed) {
      if (capacity > SIZE_MAX / 2) {
        capacity = needed;
        break;
      }
      capacity *= 2;
    }
    char *bytes = realloc(rinzu_jit_report, capacity);
    if (!bytes)
      _Exit(71);
    rinzu_jit_report = bytes;
    rinzu_jit_report_capacity = capacity;
  }
  if (vsnprintf(rinzu_jit_report + rinzu_jit_report_size, count + 1, format, arguments) != length)
    _Exit(86);
  va_end(arguments);
  rinzu_jit_report_size += count;
  return length;
}
#define printf rinzu_jit_printf
