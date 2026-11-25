#ifndef RINZU_COHORT_OBSERVER_H
#define RINZU_COHORT_OBSERVER_H
#ifndef __ASSEMBLER__
#ifndef _DARWIN_C_SOURCE
#define _DARWIN_C_SOURCE
#endif
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#ifndef _POSIX_C_SOURCE
#define _POSIX_C_SOURCE 200809L
#endif
#include <stdint.h>
#include <string.h>
#ifdef RINZU_COHORT_SDK
#include <dlfcn.h>
static inline void rinzu_cohort_event(const char *kind, uint64_t id, int64_t a, int64_t b) {
  void (*hook)(const char *, uint64_t, int64_t, int64_t) =
      (void (*)(const char *, uint64_t, int64_t, int64_t))dlsym(RTLD_DEFAULT, "rinzu_cohort_event");
  if (hook)
    hook(kind, id, a, b);
}
static inline void rinzu_cohort_callback_publisher(void (*publish)(void)) {
  void (*hook)(void (*)(void)) =
      (void (*)(void (*)(void)))dlsym(RTLD_DEFAULT, "rinzu_cohort_callback_publisher");
  if (hook)
    hook(publish);
}
static inline void rinzu_cohort_wait_helper(void) {
  void (*hook)(void) = (void (*)(void))dlsym(RTLD_DEFAULT, "rinzu_cohort_wait_helper");
  if (hook)
    hook();
}
static inline void rinzu_cohort_producer(uint64_t id) {
  void (*hook)(uint64_t) = (void (*)(uint64_t))dlsym(RTLD_DEFAULT, "rinzu_cohort_producer");
  if (hook)
    hook(id);
}
static inline void rinzu_cohort_controller(void (*finish)(void)) {
  void (*hook)(void (*)(void)) =
      (void (*)(void (*)(void)))dlsym(RTLD_DEFAULT, "rinzu_cohort_controller");
  if (hook)
    hook(finish);
}
static inline void rinzu_cohort_publish_after_selection(void (*publish)(void)) {
  void (*hook)(void (*)(void)) =
      (void (*)(void (*)(void)))dlsym(RTLD_DEFAULT, "rinzu_cohort_publish_after_selection");
  if (hook)
    hook(publish);
}
#else
void rinzu_cohort_event(const char *, uint64_t, int64_t, int64_t);
void rinzu_cohort_producer(uint64_t);
void rinzu_cohort_controller(void (*)(void));
void rinzu_cohort_publish_after_selection(void (*)(void));
#endif
void rinzu_cohort_ready_selection(uint64_t);
#define RINZU_AOT_OWNER_SELECTION_EVENT(id) rinzu_cohort_ready_selection(id)
#define RINZU_AOT_AWAIT_PARK_EVENT(scope, future) rinzu_cohort_event("await_park", scope, future, 0)
void rinzu_cohort_call(const char *, const char *);
void rinzu_cohort_callback(const char *);
#undef RINZU_NATIVE_LIFETIME_EVENT
#undef RINZU_NATIVE_CALL_EVENT
#undef RINZU_NATIVE_CALLBACK_EVENT
#undef RINZU_NATIVE_WAKE_BIND_EVENT
#undef RINZU_NATIVE_DECISION_EVENT
#undef RINZU_NATIVE_CONTROL_EVENT
#define RINZU_NATIVE_LIFETIME_EVENT(k, id, a, b) rinzu_cohort_event(k, id, a, b)
#define RINZU_NATIVE_CALL_EVENT(k, plan, symbol)                                                   \
  do {                                                                                             \
    (void)(plan);                                                                                  \
    rinzu_cohort_call(k, symbol);                                                                  \
  } while (0)
#define RINZU_NATIVE_CALLBACK_EVENT(k, symbol)                                                     \
  do {                                                                                             \
    (void)(symbol);                                                                                \
    rinzu_cohort_callback(k);                                                                      \
  } while (0)
#define RINZU_NATIVE_WAKE_BIND_EVENT(future, target)                                               \
  rinzu_cohort_event("wake_bind", (future)->identity, (intptr_t)(target), 0)
#define RINZU_NATIVE_DECISION_EVENT(future, decision)                                              \
  do {                                                                                             \
    if ((future)->operation == RINZU_ASYNC_COMPLETION)                                             \
      rinzu_cohort_event(                                                                          \
          "receipt_decision", (future)->identity,                                                  \
          (decision) == RINZU_ASYNC_CANCELLED ? 1 : ((decision) == RINZU_ASYNC_TIMED_OUT ? 2 : 0), \
          0);                                                                                      \
  } while (0)
#define RINZU_NATIVE_CONTROL_EVENT(kind, future, value)                                            \
  do {                                                                                             \
    if ((future)->operation == RINZU_ASYNC_COMPLETION)                                             \
      rinzu_cohort_event(kind, (future)->identity, value, 0);                                      \
  } while (0)
#endif
#endif
