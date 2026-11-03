/* Private protocol measurements supplement the actual managed SDK timings. */
#include "rinzu/owner_dispatcher.h"
#include "rinzu/callback_entry.h"
#include <CoreFoundation/CoreFoundation.h>
#include <assert.h>
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdio.h>
#include <unistd.h>
enum { PRODUCERS = 8, REQUESTS = 128, PULSES = 128, ENTRIES = 1537 };
static uint64_t now(void) {
  struct timespec value;
  rinzu_foreign_time(&value);
  return (uint64_t)value.tv_sec * UINT64_C(1000000000) + (uint64_t)value.tv_nsec;
}
static uint64_t nanos(struct timespec value) {
  return (uint64_t)value.tv_sec * UINT64_C(1000000000) + (uint64_t)value.tv_nsec;
}
static rinzu_foreign_queue_t *queue;
static pthread_mutex_t pulse_gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t pulse_changed = PTHREAD_COND_INITIALIZER;
static uint64_t origin_ns[PRODUCERS], receipt_ns[PRODUCERS];
static void notify(void *raw) {
  (void)raw;
  assert(!pthread_mutex_lock(&pulse_gate));
  assert(!pthread_cond_signal(&pulse_changed));
  assert(!pthread_mutex_unlock(&pulse_gate));
}
static void *producer(void *raw) {
  size_t id = (size_t)raw;
  for (size_t index = 0; index < REQUESTS; index++) {
    uint64_t word = id * REQUESTS + index;
    rinzu_foreign_record_t *record = rinzu_foreign_record_new(queue, 1, 1, &word, 1, NULL, NULL);
    uint64_t published = now();
    rinzu_foreign_publish(record);
    assert(rinzu_foreign_receive(record) == word + 1);
    uint64_t received = now();
    uint64_t dequeued = nanos(record->dequeued_at), completed = nanos(record->completed_at);
    assert(published <= dequeued && dequeued <= completed && completed <= received);
    origin_ns[id] += dequeued - published;
    receipt_ns[id] += received - completed;
    rinzu_foreign_release(record);
  }
  return NULL;
}
static void queue_measurement(void) {
  queue = rinzu_foreign_new(notify, NULL);
  pthread_t threads[PRODUCERS];
  for (size_t id = 0; id < PRODUCERS; id++)
    assert(!pthread_create(&threads[id], NULL, producer, (void *)id));
  size_t handled = 0;
  while (handled < PRODUCERS * REQUESTS) {
    rinzu_foreign_record_t *record = rinzu_foreign_pop(queue);
    if (record) {
      assert(record->sequence == ++handled);
      rinzu_foreign_complete(record, record->words[0] + 1);
      rinzu_foreign_release(record);
    } else {
      assert(!pthread_mutex_lock(&pulse_gate));
      while (!rinzu_foreign_stats(queue).queued)
        assert(!pthread_cond_wait(&pulse_changed, &pulse_gate));
      assert(!pthread_mutex_unlock(&pulse_gate));
    }
  }
  uint64_t origin = 0, receipt = 0;
  for (size_t id = 0; id < PRODUCERS; id++) {
    assert(!pthread_join(threads[id], NULL));
    origin += origin_ns[id];
    receipt += receipt_ns[id];
  }
  rinzu_foreign_stats_t stats = rinzu_foreign_stats(queue);
  assert(stats.created == handled && stats.published == handled && stats.received == handled &&
         stats.retired == handled);
  assert(!stats.live && !stats.bytes && !stats.queued && !stats.wake_pins);
  printf("RINZU_PROTOCOL_PERF=%llu,%llu,%zu,%zu,%zu,%zu,%zu,%zu,%zu,%zu\n",
         (unsigned long long)origin, (unsigned long long)receipt, handled, stats.peak_live,
         stats.peak_bytes, stats.peak_queued, stats.live, stats.bytes, stats.queued,
         stats.wake_pins);
  rinzu_foreign_dispose(queue);
  queue = NULL;
}
typedef struct {
  rinzu_owner_dispatcher_t *dispatcher;
  _Atomic unsigned pending;
  _Atomic uint64_t requested;
  size_t turns;
  uint64_t latency;
} Wake;
static int ready(void *raw, unsigned lane) {
  Wake *wake = raw;
  return lane == 1 && atomic_load(&wake->pending) == 1;
}
static int step(void *raw, unsigned lane) {
  Wake *wake = raw;
  assert(pthread_main_np());
  if (!ready(raw, lane))
    return 0;
  wake->latency += now() - atomic_load(&wake->requested);
  wake->turns++;
  atomic_store(&wake->pending, 2);
  if (wake->turns == PULSES)
    CFRunLoopStop(CFRunLoopGetCurrent());
  return 1;
}
static void *waker(void *raw) {
  Wake *wake = raw;
  for (size_t index = 0; index < PULSES; index++) {
    atomic_store(&wake->requested, now());
    atomic_store(&wake->pending, 1);
    rinzu_owner_dispatcher_wake(wake->dispatcher);
    while (atomic_load(&wake->pending) != 2)
      sched_yield();
  }
  return NULL;
}
static void wake_measurement(void) {
  rinzu_foreign_queue_t *empty = rinzu_foreign_new(NULL, NULL);
  rinzu_foreign_attach_main(empty);
  Wake wake = {0};
  wake.dispatcher = rinzu_owner_dispatcher_new(empty, step, ready, NULL, &wake);
  pthread_t thread;
  assert(!pthread_create(&thread, NULL, waker, &wake));
  (void)CFRunLoopRunInMode(kCFRunLoopDefaultMode, 10, 0);
  assert(!pthread_join(thread, NULL));
  rinzu_owner_dispatcher_stats_t stats = rinzu_owner_dispatcher_stats(wake.dispatcher);
  assert(wake.turns == PULSES && stats.managed == PULSES && !stats.foreign &&
         stats.max_activations <= 64);
  printf("RINZU_OWNER_WAKE_PERF=%llu,%zu,%llu,%llu,%llu\n", (unsigned long long)wake.latency,
         wake.turns, (unsigned long long)stats.wakes, (unsigned long long)stats.signals,
         (unsigned long long)stats.turns);
  rinzu_owner_dispatcher_dispose(wake.dispatcher);
  rinzu_foreign_dispose(empty);
}
static uint64_t callback(void *raw, const unsigned char *input) {
  assert(raw && input && input[0] == 42);
  return *(uint64_t *)raw;
}
static void template_measurement(void) {
  const void *identities[ENTRIES];
  uint64_t value = 42;
  unsigned char input[1] = {42};
  uint64_t elapsed = 0;
  for (size_t index = 0; index < ENTRIES; index++) {
    uint64_t begin = now();
    rinzu_callback_pointer_u64_entry_t entry = rinzu_callback_pointer_u64_prepare(callback, &value);
    elapsed += now() - begin;
    identities[index] = (const void *)entry.entry;
    assert(entry.entry(input) == 42);
    assert(rinzu_callback_entry_close(entry.control));
    for (size_t previous = 0; previous < index; previous++)
      assert(identities[previous] != identities[index]);
  }
  rinzu_callback_stats_t stats = rinzu_callback_entry_stats();
  assert(stats.entries == ENTRIES && stats.closed_entries == ENTRIES && stats.blocks > 1);
  assert(!stats.live_connections && !stats.in_flight && !stats.live_diagnostics &&
         !stats.diagnostic_bytes);
  printf("RINZU_TEMPLATE_PERF=%llu,%zu,%zu,%zu,%zu,%zu,%zu,%zu,%zu,%zu,%zu,%zu\n",
         (unsigned long long)elapsed, stats.entries, stats.blocks, stats.virtual_bytes,
         stats.control_bytes, stats.closed_entries, stats.tombstone_bytes,
         stats.block_metadata_bytes, stats.live_connections, stats.in_flight,
         stats.live_diagnostics, stats.diagnostic_bytes);
}
int main(void) {
  alarm(20);
  assert(pthread_main_np());
  queue_measurement();
  wake_measurement();
  template_measurement();
  puts("PASS private native performance and resource baseline");
  return 0;
}
