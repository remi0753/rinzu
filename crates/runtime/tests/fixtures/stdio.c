#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/uio.h>
#include <unistd.h>

static size_t calls;
static int zero_next, would_block_next;
static int interrupted(void) {
  if (zero_next) {
    zero_next = 0;
    return 1;
  }
  if (would_block_next || calls++ == 0) {
    errno = would_block_next ? EAGAIN : EINTR;
    would_block_next = 0;
    return -1;
  }
  return 0;
}
static ssize_t fixture_write(int fd, const void *bytes, size_t count) {
  int injected = interrupted();
  if (injected) return injected < 0 ? -1 : 0;
  return write(fd, bytes, count > 3 ? 3 : count);
}
static ssize_t fixture_writev(int fd, const struct iovec *parts, int count) {
  int injected = interrupted();
  if (injected) return injected < 0 ? -1 : 0;
  assert(count >= 1 && count <= 2);
  struct iovec partial[2];
  size_t remaining = 3;
  int used = 0;
  while (used < count && remaining) {
    partial[used] = parts[used];
    if (partial[used].iov_len > remaining) partial[used].iov_len = remaining;
    remaining -= partial[used].iov_len;
    used++;
  }
  return writev(fd, partial, used);
}
#define write fixture_write
#define writev fixture_writev
#include "../../src/stdio.c"
#undef write
#undef writev

typedef struct {
  int fd;
  size_t length;
  unsigned char *bytes;
} Reader;
static void *read_all(void *raw) {
  Reader *reader = raw;
  for (;;) {
    ssize_t count = read(reader->fd, reader->bytes + reader->length, 511);
    if (count == 0)
      break;
    assert(count > 0);
    reader->length += (size_t)count;
  }
  assert(!close(reader->fd));
  return NULL;
}
typedef struct {
  int fd;
  unsigned char marker;
} Writer;
static void *write_records(void *raw) {
  Writer *writer = raw;
  unsigned char record[150];
  memset(record, writer->marker, sizeof record);
  for (size_t call = 0; call < 50; ++call)
    assert(!rinzu_stdio_write(writer->fd, record, sizeof record, 1));
  return NULL;
}
static void preserve_sigpipe(int previously_blocked) {
  sigset_t pipe_signal, before, after, pending;
  sigemptyset(&pipe_signal);
  sigaddset(&pipe_signal, SIGPIPE);
  assert(!pthread_sigmask(previously_blocked ? SIG_BLOCK : SIG_UNBLOCK, &pipe_signal, &before));
  int fds[2];
  assert(!pipe(fds));
  assert(!close(fds[0]));
  assert(rinzu_stdio_write(fds[1], (const unsigned char *)"x", 1, 1) == EPIPE);
  assert(!pthread_sigmask(SIG_BLOCK, NULL, &after));
  assert(sigismember(&after, SIGPIPE) == previously_blocked);
  assert(!sigpending(&pending));
  assert(!sigismember(&pending, SIGPIPE));
  assert(!close(fds[1]));
  assert(!pthread_sigmask(SIG_SETMASK, &before, NULL));
}
int main(void) {
  struct sigaction action = {0}, current;
  action.sa_handler = SIG_DFL;
  sigemptyset(&action.sa_mask);
  assert(!sigaction(SIGPIPE, &action, NULL));
  unsigned char text[] = "日本語\0🙂";
  int fds[2];
  assert(!pipe(fds));
  assert(!rinzu_stdio_write(fds[1], text, sizeof text - 1, 1));
  assert(calls > 2);
  assert(!rinzu_stdio_write(fds[1], NULL, 0, 0));
  assert(!rinzu_stdio_write(fds[1], NULL, 0, 1));
  assert(!close(fds[1]));
  unsigned char result[100];
  ssize_t length = read(fds[0], result, sizeof result);
  assert(length == (ssize_t)sizeof text + 1);
  assert(!memcmp(result, text, sizeof text - 1));
  assert(result[sizeof text - 1] == '\n' && result[sizeof text] == '\n');
  assert(!close(fds[0]));

  for (size_t count = 1; count <= 7; ++count) {
    assert(!pipe(fds));
    would_block_next = 1;
    assert(!rinzu_stdio_write(fds[1], text, count, 1));
    assert(!close(fds[1]));
    assert(read(fds[0], result, sizeof result) == (ssize_t)count + 1);
    assert(!memcmp(result, text, count) && result[count] == '\n');
    assert(!close(fds[0]));
  }
  assert(!pipe(fds));
  for (int newline = 0; newline <= 1; ++newline) {
    zero_next = 1;
    assert(rinzu_stdio_write(fds[1], text, 1, newline) == EIO);
  }
  assert(rinzu_stdio_write(fds[1], NULL, 1, 1) == EINVAL);
  assert(rinzu_stdio_write(fds[1], text, 1, 2) == EINVAL);
  assert(!close(fds[0]) && !close(fds[1]));

  const size_t large = 128 * 1024;
  unsigned char *payload = malloc(large), *copy = malloc(large + 1024);
  assert(payload && copy);
  for (size_t i = 0; i < large; ++i)
    payload[i] = (unsigned char)i;
  assert(!pipe(fds));
  int flags = fcntl(fds[1], F_GETFL);
  assert(!fcntl(fds[1], F_SETFL, flags | O_NONBLOCK));
  flags = fcntl(fds[1], F_GETFL);
  assert(flags & O_NONBLOCK);
  Reader reader = {fds[0], 0, copy};
  pthread_t reading;
  assert(!pthread_create(&reading, NULL, read_all, &reader));
  assert(!rinzu_stdio_write(fds[1], payload, large, 1));
  int status_flags = O_ACCMODE | O_NONBLOCK | O_APPEND | O_ASYNC;
  assert((fcntl(fds[1], F_GETFL) & status_flags) == (flags & status_flags));
  assert(!close(fds[1]));
  assert(!pthread_join(reading, NULL));
  assert(reader.length == large + 1 && !memcmp(payload, copy, large) && copy[large] == '\n');
  free(payload);
  free(copy);

  assert(!pipe(fds));
  unsigned char *records = malloc(8 * 50 * 151 + 1024);
  assert(records);
  reader = (Reader){fds[0], 0, records};
  assert(!pthread_create(&reading, NULL, read_all, &reader));
  pthread_t writers[8];
  Writer arguments[8];
  for (size_t i = 0; i < 8; ++i) {
    arguments[i] = (Writer){fds[1], (unsigned char)('A' + i)};
    assert(!pthread_create(&writers[i], NULL, write_records, &arguments[i]));
  }
  for (size_t i = 0; i < 8; ++i)
    assert(!pthread_join(writers[i], NULL));
  assert(!close(fds[1]));
  assert(!pthread_join(reading, NULL));
  assert(reader.length == 8 * 50 * 151);
  size_t totals[8] = {0};
  for (size_t offset = 0; offset < reader.length; offset += 151) {
    unsigned char marker = records[offset];
    assert(marker >= 'A' && marker <= 'H');
    for (size_t i = 0; i < 150; ++i)
      assert(records[offset + i] == marker);
    assert(records[offset + 150] == '\n');
    totals[marker - 'A']++;
  }
  for (size_t i = 0; i < 8; ++i)
    assert(totals[i] == 50);
  free(records);

  preserve_sigpipe(0);
  preserve_sigpipe(1);
  assert(!sigaction(SIGPIPE, NULL, &current));
  assert(current.sa_handler == SIG_DFL);
  sigset_t blocked, previous, pending;
  sigemptyset(&blocked);
  sigaddset(&blocked, SIGPIPE);
  assert(!pthread_sigmask(SIG_BLOCK, &blocked, &previous));
  assert(!raise(SIGPIPE));
  assert(!pipe(fds));
  assert(!close(fds[0]));
  assert(rinzu_stdio_write(fds[1], (const unsigned char *)"x", 1, 0) == EPIPE);
  assert(rinzu_stdio_write(fds[1], (const unsigned char *)"x", 1, 1) == EPIPE);
  assert(!sigpending(&pending));
  assert(sigismember(&pending, SIGPIPE));
  int number;
  assert(!sigwait(&blocked, &number));
  assert(number == SIGPIPE);
  assert(!close(fds[1]));
  assert(!pthread_sigmask(SIG_SETMASK, &previous, NULL));
  assert(rinzu_stdio_write(-1, (const unsigned char *)"x", 1, 0) == EBADF);
  assert(rinzu_stdio_write(-1, (const unsigned char *)"x", 1, 1) == EBADF);
#ifdef __APPLE__
  assert(!pipe(fds));
  assert(!close(fds[0]));
  for (int setting = 0; setting <= 1; ++setting) {
    assert(!fcntl(fds[1], F_SETNOSIGPIPE, setting));
    for (int newline = 0; newline <= 1; ++newline) {
      assert(rinzu_stdio_write(fds[1], text, 1, newline) == EPIPE);
      assert(fcntl(fds[1], F_GETNOSIGPIPE) == setting);
    }
  }
  assert(!close(fds[1]));
#endif
  puts("PASS stdout UTF-8/NUL/EINTR/partial/nonblocking/serialization/SIGPIPE");
  return 0;
}
