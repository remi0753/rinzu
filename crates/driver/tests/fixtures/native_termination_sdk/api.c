#include "api.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <pthread.h>
#include <signal.h>
#include <string.h>
#include <time.h>
#include <errno.h>
void termination_mark(int32_t marker) {
  char line[64];
  int length = snprintf(line, sizeof(line), "TERMINATION_MARK=%d\n", marker);
  assert(length > 0 && (size_t)length < sizeof(line) && write(2, line, (size_t)length) == length);
}
static void after_exit(void) {
  termination_mark(9);
}
static void entry(void) {
  static int registered;
  if (!registered) {
    assert(!atexit(after_exit));
    registered = 1;
  }
  termination_mark(0);
}
int32_t termination_scalar(int32_t (*callback)(int32_t), int32_t mode) {
  entry();
  int32_t result = callback(mode);
  termination_mark(3);
  return result;
}
int32_t termination_unit(void (*callback)(void)) {
  entry();
  callback();
  termination_mark(3);
  return 17;
}
int32_t termination_text(int32_t (*callback)(const char *), int32_t mode) {
  entry();
  int32_t result = callback(mode == 3 ? NULL : "\xff");
  termination_mark(3);
  return result;
}
int32_t termination_bytes(int32_t (*callback)(const uint8_t *)) {
  entry();
  int32_t result = callback(NULL);
  termination_mark(3);
  return result;
}
void termination_abort(void) {
  termination_mark(8);
  abort();
}
int32_t termination_mode(void) {
  const char *mode = getenv("RINZU_TERMINATION_MODE");
  assert(mode);
  return atoi(mode);
}
static int32_t forever(const char *input, const uint8_t *bytes) {
  struct sigaction action;
  sigset_t mask;
  assert(!sigaction(SIGINT, NULL, &action) && action.sa_handler == SIG_DFL);
  assert(!pthread_sigmask(0, NULL, &mask) && !sigismember(&mask, SIGINT));
  const unsigned char expected[4] = {0, 170, 0, 127};
  assert(input && !strcmp(input, "interrupt 日本語") && bytes && !memcmp(bytes, expected, 4));
  termination_mark(10);
  for (;;) {
    struct timespec wait = {0, 20000000};
    while (nanosleep(&wait, &wait))
      assert(errno == EINTR);
    assert(!strcmp(input, "interrupt 日本語") && !memcmp(bytes, expected, 4));
  }
}
int32_t termination_forever(const char *input, const uint8_t *bytes) {
  entry();
  return forever(input, bytes);
}
int32_t termination_run_forever(void (*callback)(void), const char *input, const uint8_t *bytes) {
  assert(!getenv("RINZU_INTERRUPT_PROCESS_MAIN") || pthread_main_np());
  entry();
  callback();
  return forever(input, bytes);
}
