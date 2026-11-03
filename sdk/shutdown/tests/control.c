#include "control.h"
#include <assert.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>
static volatile sig_atomic_t previous_calls;
static void previous(int number) {
  (void)number;
  previous_calls++;
}
static void *wrong_thread(void *raw) {
  (void)raw;
  assert(rinzu_shutdown_enable() == -8101);
  return NULL;
}
int main(void) {
  struct sigaction original, custom = {.sa_handler = previous};
  sigemptyset(&custom.sa_mask);
  assert(!sigaction(SIGINT, &custom, &original));
  for (unsigned i = 0; i < 20; i++) {
    assert(!rinzu_shutdown_enable());
    assert(rinzu_shutdown_enable() == -8102);
    assert(!rinzu_shutdown_signal());
    assert(!raise(SIGTERM));
    assert(rinzu_shutdown_signal() == SIGTERM);
    assert(!rinzu_shutdown_disable());
    assert(!rinzu_shutdown_disable());
    assert(!raise(SIGINT));
  }
  assert(previous_calls == 20);
  assert(!sigaction(SIGINT, &original, NULL));
  pthread_t thread;
  assert(!pthread_create(&thread, NULL, wrong_thread, NULL));
  assert(!pthread_join(thread, NULL));
  int64_t first = rinzu_shutdown_clock(), next = rinzu_shutdown_clock();
  assert(first >= 0 && next >= first);
  pid_t child = fork();
  assert(child >= 0);
  if (!child) {
    assert(!rinzu_shutdown_enable());
    assert(!raise(SIGINT));
    assert(!raise(SIGINT));
    _exit(99);
  }
  int status;
  assert(waitpid(child, &status, 0) == child);
  assert(WIFEXITED(status) && WEXITSTATUS(status) == 130);
  puts("signals:20,restored:20,wrong-main:rejected,second:130,clock:monotonic");
}
