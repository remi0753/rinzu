#include <assert.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

/* Give a process-directed SIGPIPE time to reach another thread before exit. */
static ssize_t diagnostic_write(int fd, const void *bytes, size_t length) {
  ssize_t result = write(fd, bytes, length);
  if (result < 0 && errno == EPIPE) {
    const struct timespec delay = {.tv_nsec = 20000000};
    nanosleep(&delay, NULL);
  }
  return result;
}
#define write diagnostic_write
#include "rinzu/native_fatal.h"
#undef write
static pthread_mutex_t gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t condition = PTHREAD_COND_INITIALIZER;
static int ready;
static void *locked_stdio(void *unused) {
  (void)unused;
  flockfile(stderr);
  assert(!pthread_mutex_lock(&gate));
  ready = 1;
  assert(!pthread_cond_signal(&condition));
  for (;;)
    assert(!pthread_cond_wait(&condition, &gate));
  return NULL;
}
static void cleanup(void) {
  (void)write(1, "CLEANUP_RAN\n", 12);
}
int main(int argc, char **argv) {
  assert(argc == 3);
  assert(!atexit(cleanup));
  const char *mode = argv[2];
  if (!strcmp(mode, "lock") || !strcmp(mode, "broken")) {
    pthread_t worker;
    assert(!pthread_create(&worker, NULL, locked_stdio, NULL));
    assert(!pthread_mutex_lock(&gate));
    while (!ready)
      assert(!pthread_cond_wait(&condition, &gate));
    assert(!pthread_mutex_unlock(&gate));
  }
  if (!strcmp(mode, "closed"))
    close(2);
  else if (!strcmp(mode, "full") || !strcmp(mode, "broken")) {
    int pipes[2];
    assert(!pipe(pipes));
    if (!strcmp(mode, "full")) {
      int flags = fcntl(pipes[1], F_GETFL);
      assert(flags >= 0 && !fcntl(pipes[1], F_SETFL, flags | O_NONBLOCK));
      char bytes[4096] = {0};
      while (write(pipes[1], bytes, sizeof(bytes)) > 0) {
      }
      assert(!fcntl(pipes[1], F_SETFL, flags));
    } else {
      close(pipes[0]);
      signal(SIGPIPE, SIG_DFL);
    }
    assert(dup2(pipes[1], 2) == 2);
  }
  rinzu_native_fatal(atoi(argv[1]));
}
