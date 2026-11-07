#ifndef RINZU_NATIVE_FATAL_H
#define RINZU_NATIVE_FATAL_H
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <unistd.h>

/* No allocator, stdio lock, cleanup, retry or other-thread cooperation. Only
   nonblocking pipes, sockets and terminals receive one bounded diagnostic.
   Ordinary files/devices are skipped: O_NONBLOCK cannot bound their writes. */
static __attribute__((unused, cold, noinline)) _Noreturn void rinzu_native_fatal(int code) {
  static const char invariant[] = "rinzu: native data contract violation (86)\n";
  static const char resource[] = "rinzu: required runtime resource unavailable (71)\n";
  struct stat status;
  if (!fstat(STDERR_FILENO, &status) &&
      (S_ISFIFO(status.st_mode) || (S_ISCHR(status.st_mode) && isatty(STDERR_FILENO))
#ifdef S_ISSOCK
       || S_ISSOCK(status.st_mode)
#endif
           )) {
    int flags = fcntl(STDERR_FILENO, F_GETFL);
    if (flags >= 0 && !fcntl(STDERR_FILENO, F_SETFL, flags | O_NONBLOCK)) {
      /* On macOS a closed pipe can signal another thread before _Exit. */
      struct sigaction ignored = {.sa_handler = SIG_IGN};
      sigemptyset(&ignored.sa_mask);
      if (!sigaction(SIGPIPE, &ignored, NULL)) {
        const char *message = code == 71 ? resource : invariant;
        size_t length = code == 71 ? sizeof(resource) - 1 : sizeof(invariant) - 1;
        (void)write(STDERR_FILENO, message, length);
      }
    }
  }
  _Exit(code == 71 ? 71 : 86);
}
static inline _Noreturn void rinzu_interop_fatal(void) {
  rinzu_native_fatal(86);
}
static inline _Noreturn void rinzu_resource_fatal(void) {
  rinzu_native_fatal(71);
}
#endif
