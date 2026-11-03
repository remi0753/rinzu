#include <assert.h>
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
static int fail_allocation;
static void *probe_calloc(size_t count, size_t size) {
  return fail_allocation ? NULL : calloc(count, size);
}
#define calloc probe_calloc
#define RINZU_FOREIGN_TESTING 1
#include "rinzu/foreign_queue.h"
static rinzu_foreign_queue_t *queue;
static _Atomic size_t receipts, wakes;
static pthread_mutex_t ready_gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t ready_cond = PTHREAD_COND_INITIALIZER;
static unsigned ready, begin_receive;
static void wake(void *context) {
  rinzu_foreign_queue_t *q = context;
  assert(q == queue);
  assert(!rinzu_foreign_held_locks);
  atomic_fetch_add(&wakes, 1);
}
static void receipt(void *context) {
  rinzu_foreign_record_t *record = context;
  assert(record->state == RINZU_FOREIGN_RECEIVED);
  assert(!rinzu_foreign_held_locks);
  atomic_fetch_add(&receipts, 1);
}
static void *producer(void *raw) {
  uintptr_t producer = (uintptr_t)raw;
  rinzu_foreign_record_t *records[512];
  for (size_t index = 0; index < 512; ++index) {
    uint64_t words[2] = {producer * 512 + index, UINT64_C(0x7ff8123456789abc)};
    records[index] = rinzu_foreign_record_new(queue, producer + 1, 7, words, 2, receipt, (void *)1);
    records[index]->receipt_context = records[index];
    rinzu_foreign_publish(records[index]);
  }
  assert(!pthread_mutex_lock(&ready_gate));
  ready++;
  assert(!pthread_cond_broadcast(&ready_cond));
  while (!begin_receive)
    assert(!pthread_cond_wait(&ready_cond, &ready_gate));
  assert(!pthread_mutex_unlock(&ready_gate));
  for (size_t index = 0; index < 512; ++index) {
    rinzu_foreign_record_t *record = records[index];
    /* Spurious signals do not complete a request, and completion before wait
       is read from the predicate rather than depending on a remembered wake. */
    assert(!pthread_cond_signal(&record->result_changed));
    assert(rinzu_foreign_receive(record) == ((producer * 512 + index) ^ UINT64_C(0x55aa)));
    assert(atomic_load(&record->references) == 1);
    rinzu_foreign_release(record);
  }
  return NULL;
}
static void *bad_owner(void *raw) {
  rinzu_foreign_record_t *record = raw;
  if (record)
    rinzu_foreign_complete(record, 0);
  else
    (void)rinzu_foreign_pop(queue);
  return NULL;
}
static void *receive_first(void *raw) {
  (void)raw;
  uint64_t word = 123;
  rinzu_foreign_record_t *record =
      rinzu_foreign_record_new(queue, 1, 8, &word, 1, receipt, (void *)1);
  record->receipt_context = record;
  rinzu_foreign_publish(record);
  assert(rinzu_foreign_receive(record) == 123);
  rinzu_foreign_release(record);
  return NULL;
}
int main(int argc, char **argv) {
  assert(argc == 2);
  const char *mode = argv[1];
  if (!strcmp(mode, "allocate-queue"))
    fail_allocation = 1;
  queue = rinzu_foreign_new(wake, NULL);
  queue->wake_context = queue;
  if (strcmp(mode, "normal") != 0) {
    if (!strcmp(mode, "foreign-pop")) {
      pthread_t thread;
      assert(!pthread_create(&thread, NULL, bad_owner, NULL));
      assert(!pthread_join(thread, NULL));
      return 90;
    }
    if (!strcmp(mode, "allocate-record"))
      fail_allocation = 1;
    uint64_t word = 41;
    rinzu_foreign_record_t *record = rinzu_foreign_record_new(
        queue, !strcmp(mode, "bad-identity") ? 0 : 1, !strcmp(mode, "bad-generation") ? 0 : 1,
        &word, !strcmp(mode, "overflow") ? SIZE_MAX : 1, NULL, NULL);
    if (!strcmp(mode, "prepared-release")) {
      rinzu_foreign_release(record);
      return 90;
    }
    rinzu_foreign_publish(record);
    if (!strcmp(mode, "queued-release")) {
      rinzu_foreign_release(record);
      rinzu_foreign_release(record);
      return 90;
    }
    if (!strcmp(mode, "duplicate-publish")) {
      rinzu_foreign_publish(record);
      return 90;
    }
    if (!strcmp(mode, "drop-live")) {
      rinzu_foreign_dispose(queue);
      return 90;
    }
    assert(rinzu_foreign_pop(queue) == record);
    if (!strcmp(mode, "foreign-complete")) {
      pthread_t thread;
      assert(!pthread_create(&thread, NULL, bad_owner, record));
      assert(!pthread_join(thread, NULL));
      return 90;
    }
    rinzu_foreign_complete(record, 42);
    if (!strcmp(mode, "duplicate-complete")) {
      rinzu_foreign_complete(record, 42);
      return 90;
    }
    (void)rinzu_foreign_receive(record);
    if (!strcmp(mode, "duplicate-receive")) {
      (void)rinzu_foreign_receive(record);
      return 90;
    }
    return 90;
  }
  pthread_t threads[8];
  for (uintptr_t index = 0; index < 8; ++index)
    assert(!pthread_create(&threads[index], NULL, producer, (void *)index));
  assert(!pthread_mutex_lock(&ready_gate));
  while (ready != 8)
    assert(!pthread_cond_wait(&ready_cond, &ready_gate));
  assert(!pthread_mutex_unlock(&ready_gate));
  rinzu_foreign_stats_t stats = rinzu_foreign_stats(queue);
  assert(stats.live == 4096 && stats.queued == 4096 && stats.received == 0 && stats.retired == 0);
  uint64_t sequence = 0;
  while (queue->head) {
    rinzu_foreign_record_t *record = rinzu_foreign_pop(queue);
    assert(record->sequence == ++sequence);
    assert(record->words[1] == UINT64_C(0x7ff8123456789abc));
    assert(atomic_load(&record->references) == 2);
    rinzu_foreign_complete(record, record->words[0] ^ UINT64_C(0x55aa));
    rinzu_foreign_release(record);
  }
  stats = rinzu_foreign_stats(queue);
  assert(stats.live == 4096 && !stats.queued && !stats.received && !stats.retired);
  assert(atomic_load(&receipts) == 0);
  assert(!pthread_mutex_lock(&ready_gate));
  begin_receive = 1;
  assert(!pthread_cond_broadcast(&ready_cond));
  assert(!pthread_mutex_unlock(&ready_gate));
  for (size_t index = 0; index < 8; ++index)
    assert(!pthread_join(threads[index], NULL));
  stats = rinzu_foreign_stats(queue);
  assert(!stats.live && !stats.queued && !stats.bytes);
  assert(stats.created == 4096 && stats.published == 4096 && stats.received == 4096 &&
         stats.retired == 4096);
  assert(atomic_load(&receipts) == 4096 && atomic_load(&wakes) == 12288);
  pthread_t first;
  assert(!pthread_create(&first, NULL, receive_first, NULL));
  rinzu_foreign_record_t *record = NULL;
  while (!(record = rinzu_foreign_pop(queue)))
    sched_yield();
  rinzu_foreign_complete(record, 123);
  assert(!pthread_join(first, NULL));
  /* Native receipt/admission settlement alone cannot reclaim Owner's ref. */
  stats = rinzu_foreign_stats(queue);
  assert(stats.live == 1 && stats.retired == 4096 && stats.received == 4097);
  assert(atomic_load(&record->references) == 1 && record->admission_settled);
  rinzu_foreign_release(record);
  stats = rinzu_foreign_stats(queue);
  assert(!stats.live && !stats.bytes && stats.retired == 4097);
  assert(atomic_load(&receipts) == 4097 && atomic_load(&wakes) == 12291);
  rinzu_foreign_dispose(queue);
  puts("PASS 4096 admitted records and FIFO/result/reference settlement");
  return 0;
}
