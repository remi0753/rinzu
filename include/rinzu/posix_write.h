#ifndef RINZU_POSIX_WRITE_H
#define RINZU_POSIX_WRITE_H
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdlib.h>
#include <sys/uio.h>
#include <unistd.h>

/* The caller owns this endpoint exclusively. Darwin sends pipe SIGPIPE to the
   process, so blocking just this thread cannot protect other running threads.
   Temporarily suppress it on the shared file description, then restore it. */
typedef struct {
#ifdef __APPLE__
  int previous;
#else
  sigset_t mask, previous;
  int was_pending;
#endif
} rinzu_posix_write_guard_t;

static inline int rinzu_posix_write_begin(int fd, rinzu_posix_write_guard_t *guard) {
#ifdef __APPLE__
  guard->previous = fcntl(fd, F_GETNOSIGPIPE);
  if (guard->previous < 0 || (!guard->previous && fcntl(fd, F_SETNOSIGPIPE, 1)))
    return -1;
#else
  (void)fd;
  sigset_t pending;
  sigemptyset(&guard->mask);
  sigaddset(&guard->mask, SIGPIPE);
  int status = pthread_sigmask(SIG_BLOCK, &guard->mask, &guard->previous);
  if (status) {
    errno = status;
    return -1;
  }
  if (sigpending(&pending))
    _Exit(86);
  guard->was_pending = sigismember(&pending, SIGPIPE);
#endif
  return 0;
}

static inline ssize_t rinzu_posix_write_end(int fd, const rinzu_posix_write_guard_t *guard,
                                            ssize_t result) {
  int saved_errno = errno;
#ifdef __APPLE__
  if (!guard->previous && fcntl(fd, F_SETNOSIGPIPE, 0))
    _Exit(86);
#else
  (void)fd;
  sigset_t pending;
  if (result < 0 && saved_errno == EPIPE && !guard->was_pending) {
    if (sigpending(&pending))
      _Exit(86);
    if (sigismember(&pending, SIGPIPE)) {
      int number;
      if (sigwait(&guard->mask, &number) || number != SIGPIPE)
        _Exit(86);
    }
  }
  if (pthread_sigmask(SIG_SETMASK, &guard->previous, NULL))
    _Exit(86);
#endif
  errno = saved_errno;
  return result;
}

static inline ssize_t rinzu_posix_write(int fd, const void *bytes, size_t length) {
  rinzu_posix_write_guard_t guard;
  if (rinzu_posix_write_begin(fd, &guard))
    return -1;
  return rinzu_posix_write_end(fd, &guard, write(fd, bytes, length));
}

static inline ssize_t rinzu_posix_writev(int fd, const struct iovec *parts, int count) {
  rinzu_posix_write_guard_t guard;
  if (rinzu_posix_write_begin(fd, &guard))
    return -1;
  return rinzu_posix_write_end(fd, &guard, writev(fd, parts, count));
}
#endif
