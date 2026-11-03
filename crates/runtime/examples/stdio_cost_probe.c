/* Build the identical probe against frozen/current stdio.c and posix_write.h.
 * Timings use real writes to /dev/null; syscall counters run in a separate pass. */
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/uio.h>
#include <time.h>
#include <unistd.h>

static volatile int counting;
static uint64_t write_calls, fcntl_calls;
static ssize_t counted_write(int fd, const void *bytes, size_t length) {
  if (counting) write_calls++;
  return write(fd, bytes, length);
}
static __attribute__((unused)) ssize_t counted_writev(int fd, const struct iovec *parts, int count) {
  if (counting) write_calls++;
  return writev(fd, parts, count);
}
#ifdef __APPLE__
static int counted_fcntl(int fd, int command, ...) {
  if (counting) fcntl_calls++;
  if (command == F_GETNOSIGPIPE) return fcntl(fd, command);
  assert(command == F_SETNOSIGPIPE);
  va_list arguments;
  va_start(arguments, command);
  int value = va_arg(arguments, int);
  va_end(arguments);
  return fcntl(fd, command, value);
}
#define fcntl counted_fcntl
#endif
#define write counted_write
#define writev counted_writev
#ifndef RINZU_STDIO_PROBE_SOURCE
#define RINZU_STDIO_PROBE_SOURCE "../src/stdio.c"
#endif
#include RINZU_STDIO_PROBE_SOURCE
#undef write
#undef writev
#ifdef __APPLE__
#undef fcntl
#endif

static uint64_t nanos(void) {
  struct timespec now;
  assert(!clock_gettime(CLOCK_MONOTONIC, &now));
  return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}
static void run(int fd, const unsigned char *bytes, size_t length, int newline) {
  const size_t rounds = 50000;
  for (size_t i = 0; i < 2000; ++i)
    assert(!rinzu_stdio_write(fd, bytes, length, newline));
  uint64_t elapsed = 0;
  for (int diagnostic = 0; diagnostic < 2; ++diagnostic) {
    write_calls = fcntl_calls = 0;
    counting = diagnostic;
    uint64_t start = nanos();
    for (size_t i = 0; i < rounds; ++i)
      assert(!rinzu_stdio_write(fd, bytes, length, newline));
    if (!diagnostic) elapsed = nanos() - start;
    counting = 0;
  }
  printf("{\"case\":\"stdio-%zu-newline-%d\",\"operations\":%zu,\"nanoseconds\":%" PRIu64
         ",\"allocations\":0,\"allocated_bytes\":0,\"write_calls\":%" PRIu64
         ",\"fcntl_calls\":%" PRIu64 ",\"checksum\":%zu}\n",
         length, newline, rounds, elapsed, write_calls, fcntl_calls,
         (length + (size_t)newline) * rounds);
}
int main(int argc, char **argv) {
  assert(argc == 2 && !strcmp(argv[1], "stdio"));
  int fd = open("/dev/null", O_WRONLY);
  assert(fd >= 0);
  unsigned char bytes[65536];
  for (size_t i = 0; i < sizeof bytes; ++i) bytes[i] = (unsigned char)i;
  const size_t sizes[] = {0, 8, 64, 4096, 65536};
  for (size_t i = 0; i < sizeof sizes / sizeof sizes[0]; ++i)
    for (int newline = 0; newline <= 1; ++newline)
      run(fd, bytes, sizes[i], newline);
  assert(!close(fd));
}
