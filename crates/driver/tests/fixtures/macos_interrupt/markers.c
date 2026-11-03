#include <assert.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <unistd.h>

/* Appended only to a private copy of the real SDK's Objective-C translation
   unit. No replacement RunLoop or application signal handler is installed. */
void rinzu_macos_interrupt_mark(int32_t marker) {
  if (marker == 10) {
    struct sigaction action;
    sigset_t mask;
    assert(pthread_main_np());
    assert(!sigaction(SIGINT, NULL, &action) && action.sa_handler == SIG_DFL);
    assert(!pthread_sigmask(0, NULL, &mask) && !sigismember(&mask, SIGINT));
    assert([[NSApplication sharedApplication] isRunning]);
  }
  char line[64];
  int length = snprintf(line, sizeof(line), "TERMINATION_MARK=%d\n", marker);
  assert(length > 0 && (size_t)length < sizeof(line));
  assert(write(2, line, (size_t)length) == length);
}
