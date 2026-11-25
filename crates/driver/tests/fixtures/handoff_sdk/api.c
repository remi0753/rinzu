#include "observer.h"
#include "api.h"
#include "completion.h"
#include <assert.h>
#include <pthread.h>
#include <stdlib.h>
#include <sched.h>
static rinzu_native_completion_v3_t *carrier;
static pthread_t producer;
static int threaded;
static void publish(void) {
  handoff_event("publish_begin", 0, 0, 0);
  rinzu_native_completion_v2_publish(&carrier->completion);
  handoff_event("publish_end", 0, 0, 0);
}
static void *work(void *unused) {
  (void)unused;
  const int controlled = getenv("HANDOFF_CONTROL") != NULL;
  if (controlled) {
    while (!atomic_load_explicit(&carrier->completion.base.cancelled, memory_order_acquire))
      sched_yield();
  } else {
    while (!atomic_load_explicit(&carrier->completion.notification, memory_order_acquire))
      sched_yield();
  }
  publish();
  return NULL;
}
HandoffCompletion *handoff_start(size_t (*callback)(void)) {
  assert(!carrier);
  carrier = calloc(1, sizeof(*carrier));
  assert(carrier);
  rinzu_native_completion_v2_init(&carrier->completion);
  carrier->completion.base.version = 3;
  carrier->completion.base.size = sizeof(*carrier);
  carrier->result = 7183;
  handoff_event("sdk_start", 0, 0, 0);
  assert(callback() == 7183);
  if (getenv("HANDOFF_EARLY"))
    publish();
  else {
    threaded = 1;
    assert(!pthread_create(&producer, NULL, work, NULL));
  }
  return (HandoffCompletion *)carrier;
}
int handoff_finish(void) {
  if (threaded)
    assert(!pthread_join(producer, NULL));
  assert(rinzu_native_completion_is_done(&carrier->completion.base));
  handoff_event("close_admission", 0, 0, 0);
  handoff_event("carrier_free", 0, 0, 0);
  free(carrier);
  carrier = NULL;
  handoff_event("sdk_pin_free", 0, 0, 0);
  return 1;
}

int handoff_controlled(void) {
  const char *mode = getenv("HANDOFF_CONTROL");
  return mode ? atoi(mode) : 0;
}
