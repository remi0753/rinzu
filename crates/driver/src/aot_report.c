#include <stdarg.h>
#include <fcntl.h>
#ifdef RINZU_AOT_BUFFERED_REPORT
/* Reports are assembled on Main. Diagnostic emitters use raw stderr writes;
 * a FILE lock cannot protect a multi-part report from those writes. Publish
 * the report only after every emitter and physical Worker has returned. */
static char *rinzu_report_bytes;
static size_t rinzu_report_length, rinzu_report_capacity;
static int rinzu_report_active;
static void rinzu_report_begin(void) {
  rinzu_report_active = 1;
}
static int rinzu_report_printf(const char *format, ...) {
  va_list arguments, copy;
  va_start(arguments, format);
  if (!rinzu_report_active) {
    int length = vfprintf(stderr, format, arguments);
    va_end(arguments);
    return length;
  }
  va_copy(copy, arguments);
  int length = vsnprintf(NULL, 0, format, copy);
  va_end(copy);
  if (length < 0)
    _Exit(86);
  size_t count = (size_t)length;
  if (count == SIZE_MAX || rinzu_report_length > SIZE_MAX - count - 1)
    _Exit(71);
  size_t needed = rinzu_report_length + count + 1;
  if (needed > rinzu_report_capacity) {
    size_t capacity = rinzu_report_capacity ? rinzu_report_capacity : 256;
    while (capacity < needed) {
      if (capacity > SIZE_MAX / 2) {
        capacity = needed;
        break;
      }
      capacity *= 2;
    }
    char *bytes = realloc(rinzu_report_bytes, capacity);
    if (!bytes)
      _Exit(71);
    rinzu_report_bytes = bytes;
    rinzu_report_capacity = capacity;
  }
  if (vsnprintf(rinzu_report_bytes + rinzu_report_length, count + 1, format, arguments) != length)
    _Exit(86);
  va_end(arguments);
  rinzu_report_length += count;
  return length;
}
static int rinzu_report_flush(void) {
  const char *path = getenv("RINZU_EVIDENCE_OUTPUT");
  if (path && fflush(NULL))
    _Exit(86);
  int output = STDERR_FILENO;
  if (path) {
    output = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0600);
    if (output < 0)
      _Exit(86);
  }
  int failed =
      rinzu_stdio_write(output, (const unsigned char *)rinzu_report_bytes, rinzu_report_length, 0);
  if (path && failed)
    _Exit(86);
  if (path) {
    static const unsigned char complete[] = "RINZU_REPORT_COMPLETE=1\n";
    if (rinzu_stdio_write(output, complete, sizeof(complete) - 1, 0) || close(output))
      _Exit(86);
  }
  free(rinzu_report_bytes);
  rinzu_report_bytes = NULL;
  rinzu_report_length = rinzu_report_capacity = 0;
  rinzu_report_active = 0;
  return failed != 0;
}
#else
/* Compiled JIT already captures reports in its per-image native buffer. */
static void rinzu_report_begin(void) {}
static int rinzu_report_flush(void) {
  return 0;
}
#endif
