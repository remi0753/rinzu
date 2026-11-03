#ifndef RINZU_THREAD_IDENTITY_H
#define RINZU_THREAD_IDENTITY_H
#include <pthread.h>

/* Darwin libpthread implements pthread_equal as this pointer comparison.
   Keep the public API on other platforms, where pthread_t may differ. */
static inline int rinzu_thread_equal(pthread_t left, pthread_t right) {
#if defined(__APPLE__) && defined(__MACH__)
  return left == right;
#else
  return pthread_equal(left, right);
#endif
}
#endif
