/* Synchronous stdout writes shared by Rust JIT and the generated C runtime. */
#include "rinzu/posix_write.h"
#include <errno.h>
#include <limits.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static pthread_mutex_t rinzu_stdio_mutex = PTHREAD_MUTEX_INITIALIZER;

static int rinzu_stdio_retry(int fd, ssize_t written) {
  if (written == 0)
    return EIO;
  int status = errno;
  if (status == EINTR)
    return 0;
  if (status == EAGAIN || status == EWOULDBLOCK) {
    struct pollfd output = {fd, POLLOUT, 0};
    int ready;
    do {
      ready = poll(&output, 1, -1);
    } while (ready < 0 && errno == EINTR);
    if (ready < 0)
      return errno;
    return output.revents & POLLNVAL ? EBADF : 0;
  }
  return status;
}

static int rinzu_stdio_write_all(int fd, const unsigned char *bytes, size_t length) {
  while (length) {
    size_t chunk = length > (size_t)SSIZE_MAX ? (size_t)SSIZE_MAX : length;
    ssize_t written = rinzu_posix_write(fd, bytes, chunk);
    if (written > 0) {
      bytes += (size_t)written;
      length -= (size_t)written;
      continue;
    }
    int status = rinzu_stdio_retry(fd, written);
    if (status)
      return status;
  }
  return 0;
}

static int rinzu_stdio_write_line(int fd, const unsigned char *bytes, size_t length) {
  static const unsigned char lf = '\n';
  while (length) {
    size_t chunk = length > (size_t)SSIZE_MAX ? (size_t)SSIZE_MAX : length;
    struct iovec parts[2] = {{(void *)bytes, chunk}, {(void *)&lf, 1}};
    int count = chunk == length && chunk < (size_t)SSIZE_MAX ? 2 : 1;
    ssize_t written = rinzu_posix_writev(fd, parts, count);
    if (written > 0) {
      size_t payload = (size_t)written > chunk ? chunk : (size_t)written;
      bytes += payload;
      length -= payload;
      if ((size_t)written > payload)
        return 0; /* The final newline was included. */
      continue;
    }
    int status = rinzu_stdio_retry(fd, written);
    if (status)
      return status;
  }
  return rinzu_stdio_write_all(fd, &lf, 1);
}

/* The caller publishes roots and enters native-safe state before this call.
 * Only native bytes are accessed while waiting. O_NONBLOCK is preserved;
 * Darwin's shared NOSIGPIPE setting is temporarily changed and restored. */
int rinzu_stdio_write(int fd, const unsigned char *bytes, size_t length, int newline) {
  if ((length && !bytes) || (newline != 0 && newline != 1))
    return EINVAL;
  int status = pthread_mutex_lock(&rinzu_stdio_mutex);
  if (!status) {
    status = newline ? rinzu_stdio_write_line(fd, bytes, length)
                     : rinzu_stdio_write_all(fd, bytes, length);
    int unlocked = pthread_mutex_unlock(&rinzu_stdio_mutex);
    if (!status)
      status = unlocked;
  }
  return status;
}

/* Diagnostic failure never replaces the managed panic. No managed memory is
 * accessed here; callers enter native-safe state with an owned native copy. */
void rinzu_stdio_panic(const unsigned char *bytes, size_t length) {
  static const unsigned char prefix[] = "panic: ";
  if (length > SIZE_MAX - (sizeof(prefix) - 1))
    return;
  size_t total = sizeof(prefix) - 1 + length;
  unsigned char *record = malloc(total);
  if (!record)
    return;
  memcpy(record, prefix, sizeof(prefix) - 1);
  if (length)
    memcpy(record + sizeof(prefix) - 1, bytes, length);
  (void)rinzu_stdio_write(STDERR_FILENO, record, total, 1);
  free(record);
}
