#ifndef RINZU_HANDOFF_OBSERVER_H
#define RINZU_HANDOFF_OBSERVER_H
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
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
static inline void handoff_event(const char *kind, uint64_t id, int64_t a, int64_t b) {
  flockfile(stdout);
  if (!strcmp(kind, "carrier_bind")) {
    funlockfile(stdout);
    return;
  }
  if (!strcmp(kind, "admit"))
    puts("drain_receipt,0,0,0");
  if (!strcmp(kind, "record_release"))
    puts("drain_release,0,0,0");
  printf("%s,%" PRIu64 ",%" PRId64 ",%" PRId64 "\n", kind, id, a, b);
  funlockfile(stdout);
}
#undef RINZU_NATIVE_LIFETIME_EVENT
#undef RINZU_NATIVE_CALL_EVENT
#undef RINZU_NATIVE_CALLBACK_EVENT
#undef RINZU_NATIVE_DECISION_EVENT
#undef RINZU_NATIVE_CONTROL_EVENT
#define RINZU_NATIVE_LIFETIME_EVENT(kind, id, a, b) handoff_event(kind, id, a, b)
#define RINZU_NATIVE_CALL_EVENT(kind, plan, symbol)                                                \
  do {                                                                                             \
    (void)(plan);                                                                                  \
    if (!strcmp(symbol, "handoff_start"))                                                          \
      handoff_event(kind, 0, 0, 0);                                                                \
  } while (0)
#define RINZU_NATIVE_CALLBACK_EVENT(kind, symbol)                                                  \
  do {                                                                                             \
    (void)(symbol);                                                                                \
    handoff_event(kind, 0, 0, 0);                                                                  \
  } while (0)
#define RINZU_NATIVE_DECISION_EVENT(future, decision)                                              \
  do {                                                                                             \
    if ((future)->operation == RINZU_ASYNC_COMPLETION)                                             \
      handoff_event(                                                                               \
          "receipt_decision", 0,                                                                   \
          (decision) == RINZU_ASYNC_CANCELLED ? 1 : ((decision) == RINZU_ASYNC_TIMED_OUT ? 2 : 0), \
          0);                                                                                      \
  } while (0)
#define RINZU_NATIVE_CONTROL_EVENT(kind, future, value)                                            \
  do {                                                                                             \
    if ((future)->operation == RINZU_ASYNC_COMPLETION)                                             \
      handoff_event(kind, (future)->identity, value, 0);                                           \
  } while (0)
#endif

#endif
