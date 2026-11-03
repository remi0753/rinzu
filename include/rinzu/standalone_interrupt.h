#ifndef RINZU_STANDALONE_INTERRUPT_H
#define RINZU_STANDALONE_INTERRUPT_H
/* Standalone startup only. Embedded runtimes preserve the host's signal policy.
   No managed event, signal handler, forwarding or cleanup is installed. */
#if defined(__unix__) || defined(__APPLE__)
#include <pthread.h>
#include <signal.h>
#include <stdlib.h>
#include <unistd.h>
static inline void rinzu_standalone_interrupt_normalize(void) {
  struct sigaction action = {0};
  action.sa_handler = SIG_DFL;
  sigset_t interrupt;
  if (sigemptyset(&action.sa_mask) || sigaction(SIGINT, &action, NULL) || sigemptyset(&interrupt) ||
      sigaddset(&interrupt, SIGINT) || pthread_sigmask(SIG_UNBLOCK, &interrupt, NULL))
    _Exit(86);
}
#else
static inline void rinzu_standalone_interrupt_normalize(void) {}
#endif
#endif
