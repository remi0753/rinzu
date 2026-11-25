#include "cases.h"
#include "rinzu/thread_identity.h"
#include <fcntl.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

enum { MAX_NAME_BYTES = 4096, MAX_ERROR_BYTES = 16384 };

typedef struct CaseName {
  struct CaseName *next;
  size_t length;
  uint8_t bytes[];
} CaseName;

static pthread_mutex_t case_lock = PTHREAD_MUTEX_INITIALIZER;
static CaseName *names;
static uint64_t next_case;
static uint64_t active_case;
static uint64_t started_at;

__attribute__((destructor)) static void release_names(void) {
  while (names) {
    CaseName *name = names;
    names = name->next;
    free(name);
  }
}

static uint64_t monotonic_ms(void) {
  struct timespec now;
  if (clock_gettime(CLOCK_MONOTONIC, &now) != 0)
    _Exit(74);
  return (uint64_t)now.tv_sec * UINT64_C(1000) + (uint64_t)now.tv_nsec / UINT64_C(1000000);
}

static FILE *open_events(const char *path) {
  int descriptor = open(path, O_WRONLY | O_APPEND | O_NONBLOCK | O_NOFOLLOW);
  struct stat metadata;
  if (descriptor < 0 || fstat(descriptor, &metadata) != 0 || !S_ISREG(metadata.st_mode))
    _Exit(74);
  FILE *events = fdopen(descriptor, "a");
  if (!events)
    _Exit(74);
  return events;
}

static void close_event(FILE *events) {
  if (fputs("}\n", events) < 0 || ferror(events) || fclose(events) != 0)
    _Exit(74);
}

static void write_string(FILE *events, const uint8_t *text, size_t length) {
  fputc('"', events);
  for (size_t i = 0; i < length; ++i) {
    unsigned byte = text[i];
    if (byte == '"' || byte == '\\') {
      fputc('\\', events);
      fputc((int)byte, events);
    } else if (byte < 0x20) {
      fprintf(events, "\\u%04x", byte);
    } else {
      fputc((int)byte, events);
    }
  }
  fputc('"', events);
}

/* The SDK supplies valid UTF-8 from a String. */
static int has_non_whitespace(const uint8_t *text, size_t length) {
  for (size_t i = 0; i < length;) {
    uint32_t code = text[i++];
    if (code >= 0x80) {
      size_t continuation = code < 0xe0 ? 1 : (code < 0xf0 ? 2 : 3);
      code &= (1u << (6 - continuation)) - 1u;
      if (continuation > length - i)
        return 1;
      while (continuation--)
        code = (code << 6) | (text[i++] & 0x3f);
    }
    if (!((code >= 0x09 && code <= 0x0d) || code == 0x20 || code == 0x85 || code == 0xa0 ||
          code == 0x1680 || (code >= 0x2000 && code <= 0x200a) || code == 0x2028 ||
          code == 0x2029 || code == 0x202f || code == 0x205f || code == 0x3000))
      return 1;
  }
  return 0;
}

static int64_t invalid(const char *path, const char *message) {
  FILE *events = open_events(path);
  fputs("{\"event\":\"invalid\",\"message\":", events);
  write_string(events, (const uint8_t *)message, strlen(message));
  close_event(events);
  return -1;
}

static int64_t begin_case(const char *path, const uint8_t *name, size_t length) {
  if (!rinzu_thread_is_main())
    return invalid(path, "named cases must run in the test file's Main Domain");
  if (active_case)
    return invalid(path, "named cases must run one at a time");
  if (!length || length > MAX_NAME_BYTES || memchr(name, 0, length) ||
      !has_non_whitespace(name, length))
    return invalid(path, "test case name must contain text, have no NUL and fit within 4096 bytes");
  for (const CaseName *previous = names; previous; previous = previous->next) {
    if (previous->length == length && memcmp(previous->bytes, name, length) == 0)
      return invalid(path, "duplicate test case name");
  }
  CaseName *registration = malloc(sizeof(*registration) + length);
  if (!registration)
    _Exit(70);
  registration->length = length;
  memcpy(registration->bytes, name, length);
  registration->next = names;
  names = registration;

  const char *filter = getenv("RINZU_FILE_TEST_CASE");
  if (filter && (strlen(filter) != length || memcmp(filter, name, length) != 0))
    return 0;

  active_case = ++next_case;
  started_at = monotonic_ms();
  FILE *events = open_events(path);
  fprintf(events, "{\"event\":\"start\",\"sequence\":%" PRIu64 ",\"name\":", active_case);
  write_string(events, name, length);
  close_event(events);
  return 1;
}

static const char *event_path(void) {
  const char *supervisor = getenv("RINZU_FILE_TEST_SUPERVISOR");
  if (!supervisor)
    return NULL;
  char *end = NULL;
  unsigned long pid = strtoul(supervisor, &end, 10);
  return end != supervisor && !*end && pid == (unsigned long)getppid()
             ? getenv("RINZU_FILE_TEST_EVENTS")
             : NULL;
}

int64_t rinzu_testing_case_begin(const uint8_t *name, size_t length) {
  const char *path = event_path();
  if (!path)
    return -1;
  pthread_mutex_lock(&case_lock);
  int64_t result = begin_case(path, name, length);
  pthread_mutex_unlock(&case_lock);
  return result;
}

static size_t error_length(const uint8_t *text, size_t length) {
  if (length <= MAX_ERROR_BYTES)
    return length;
  length = MAX_ERROR_BYTES;
  while ((text[length] & 0xc0) == 0x80)
    --length;
  return length;
}

void rinzu_testing_case_error(const uint8_t *name, size_t name_length, const uint8_t *message,
                              size_t message_length) {
  const char *path = event_path();
  if (!path || !rinzu_thread_is_main() || !active_case)
    _Exit(74);
  pthread_mutex_lock(&case_lock);
  FILE *events = open_events(path);
  fprintf(events, "{\"event\":\"error\",\"sequence\":%" PRIu64 ",\"name\":", active_case);
  write_string(events, name, error_length(name, name_length));
  fputs(",\"message\":", events);
  write_string(events, message, error_length(message, message_length));
  if (name_length > MAX_ERROR_BYTES || message_length > MAX_ERROR_BYTES)
    fputs(",\"truncated\":true", events);
  close_event(events);
  pthread_mutex_unlock(&case_lock);
}

void rinzu_testing_case_end(int64_t failed) {
  const char *path = event_path();
  if (!path || !rinzu_thread_is_main() || !active_case)
    _Exit(74);
  pthread_mutex_lock(&case_lock);
  FILE *events = open_events(path);
  fprintf(events,
          "{\"event\":\"end\",\"sequence\":%" PRIu64 ",\"failed\":%s,\"elapsedMs\":%" PRIu64,
          active_case, failed ? "true" : "false", monotonic_ms() - started_at);
  close_event(events);
  active_case = 0;
  pthread_mutex_unlock(&case_lock);
}
