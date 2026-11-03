#include "control.h"
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
/* Explicit process-signal capability. No managed pointer enters this relay. */
static struct sigaction previous_int, previous_term;
static _Atomic int requested;
static int enabled;
/* Opt-in dev receipt, written outside the signal handler. */
static int publish_status(int active) {
  const char *path = getenv("RINZU_DEV_SIGNAL_STATUS");
  const char *nonce = getenv("RINZU_DEV_NONCE");
  if (!path && !nonce)
    return 0;
  if (!path || !nonce || strlen(nonce) != 32 || strspn(nonce, "0123456789abcdef") != 32 ||
      strlen(path) > 3500)
    return -1;
  char temporary[3584], body[256];
  int length = snprintf(body, sizeof(body),
                        "{\"schema\":\"rinzu.dev-signal-status\",\"version\":1,"
                        "\"nonce\":\"%s\",\"processId\":%ld,\"enabled\":%s}\n",
                        nonce, (long)getpid(), active ? "true" : "false");
  snprintf(temporary, sizeof(temporary), "%s.tmp-%ld", path, (long)getpid());
  int fd = open(temporary, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0600);
  if (fd < 0)
    return -1;
  int offset = 0, failed = 0;
  while (offset < length) {
    ssize_t written = write(fd, body + offset, (size_t)(length - offset));
    if (written < 0 && errno == EINTR)
      continue;
    if (written <= 0) {
      failed = 1;
      break;
    }
    offset += (int)written;
  }
  if (close(fd))
    failed = 1;
  if (!failed && rename(temporary, path))
    failed = 1;
  if (failed)
    unlink(temporary);
  return failed ? -1 : 0;
}
static void relay(int number) {
  if (atomic_exchange_explicit(&requested, number, memory_order_relaxed))
    _exit(128 + number);
}
int32_t rinzu_shutdown_enable(void) {
  if (!pthread_main_np())
    return -8101;
  if (enabled)
    return -8102;
  if (!atomic_is_lock_free(&requested))
    return -8103;
  atomic_store_explicit(&requested, 0, memory_order_relaxed);
  struct sigaction action = {.sa_handler = relay};
  sigemptyset(&action.sa_mask);
  sigaddset(&action.sa_mask, SIGINT);
  sigaddset(&action.sa_mask, SIGTERM);
  if (sigaction(SIGINT, &action, &previous_int))
    return -8104;
  if (sigaction(SIGTERM, &action, &previous_term)) {
    if (sigaction(SIGINT, &previous_int, NULL))
      _exit(70);
    return -8104;
  }
  enabled = 1;
  if (publish_status(1)) {
    if (sigaction(SIGINT, &previous_int, NULL) || sigaction(SIGTERM, &previous_term, NULL))
      _exit(70);
    enabled = 0;
    return -8105;
  }
  return 0;
}
int32_t rinzu_shutdown_disable(void) {
  if (!pthread_main_np())
    return -8101;
  if (!enabled)
    return 0;
  if (sigaction(SIGINT, &previous_int, NULL) || sigaction(SIGTERM, &previous_term, NULL))
    _exit(70);
  enabled = 0;
  atomic_store_explicit(&requested, 0, memory_order_relaxed);
  return publish_status(0) ? -8105 : 0;
}
int32_t rinzu_shutdown_signal(void) {
  if (!pthread_main_np())
    return -8101;
  return atomic_load_explicit(&requested, memory_order_relaxed);
}
int64_t rinzu_shutdown_clock(void) {
  struct timespec time;
  int64_t milliseconds;
  if (clock_gettime(CLOCK_MONOTONIC, &time) ||
      __builtin_mul_overflow((int64_t)time.tv_sec, INT64_C(1000), &milliseconds) ||
      __builtin_add_overflow(milliseconds, (int64_t)time.tv_nsec / 1000000, &milliseconds))
    return -1;
  return milliseconds;
}
