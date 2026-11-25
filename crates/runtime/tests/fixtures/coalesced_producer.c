#include <stdint.h>

typedef void (*Observer)(const char *, int64_t);
static _Thread_local Observer observer;

#define RINZU_NATIVE_LIFETIME_EVENT(kind, id, a, b)                                                \
  do {                                                                                             \
    (void)(id);                                                                                    \
    (void)(b);                                                                                     \
    observer(kind, a);                                                                             \
  } while (0)
#include "rinzu/native_completion.h"

rinzu_native_completion_wake_t *capture_publication(rinzu_native_completion_v2_t *carrier,
                                                    Observer emit) {
  observer = emit;
  rinzu_native_completion_wake_t *target = rinzu_native_completion_v2_publish_target(carrier);
  observer = NULL;
  return target;
}
