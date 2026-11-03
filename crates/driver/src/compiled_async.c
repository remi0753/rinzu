#include <stdint.h>
#include <stdlib.h>
#include <stdio.h>
#include <stdarg.h>
#include <string.h>
#include <unistd.h>
#include <time.h>
typedef struct { uint64_t native_startup, main; } rinzu_jit_timing_t;
static rinzu_jit_timing_t *rinzu_jit_timing;
static uint64_t rinzu_jit_clock(void) {
  struct timespec now;
  if (clock_gettime(CLOCK_MONOTONIC, &now)) return 0;
  return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}
/* JIT_ENTRY_COUNTS */
static uintptr_t rinzu_jit_functions[RINZU_JIT_FUNCTION_COUNT ? RINZU_JIT_FUNCTION_COUNT : 1];
static uintptr_t rinzu_jit_stubs[RINZU_JIT_STUB_COUNT ? RINZU_JIT_STUB_COUNT : 1];
static uint64_t rinzu_jit_cancel_after, rinzu_jit_shutdown_after;
static const char *rinzu_jit_directory;
static char *rinzu_jit_report;
static size_t rinzu_jit_report_size, rinzu_jit_report_capacity;
static int rinzu_jit_used;
static uint64_t (*rinzu_jit_context_identity)(void);
#define RINZU_CONTEXT_IDENTITY() (rinzu_jit_context_identity ? rinzu_jit_context_identity() : (_Exit(86), UINT64_C(0)))
static int rinzu_jit_printf(const char *format, ...) {
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
