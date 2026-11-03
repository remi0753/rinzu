#include "rinzu/callback_entry.h"
#include <assert.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

enum { ROUNDS = 8192, WORKERS = 4, PER_WORKER = 2048 };
static uint64_t body(void *raw, const unsigned char *data) {
  return *(uint64_t *)raw + *data;
}
static uint64_t closing(void *raw, const unsigned char *data) {
  rinzu_callback_pointer_u64_entry_t *entry = raw;
  assert(!rinzu_callback_entry_close(entry->control));
  assert(!rinzu_callback_entry_retire(&entry->control));
  return *data;
}
static void *worker(void *raw) {
  uintptr_t *addresses = raw;
  uint64_t context = 17;
  unsigned char input = 3;
  for (size_t index = 0; index < PER_WORKER; ++index) {
    rinzu_callback_pointer_u64_entry_t entry = rinzu_callback_pointer_u64_prepare(body, &context);
    addresses[index] = (uintptr_t)entry.entry;
    assert(entry.entry(&input) == 20);
    assert(rinzu_callback_entry_close(entry.control));
    assert(rinzu_callback_entry_retire(&entry.control));
    assert(!entry.control && rinzu_callback_entry_retire(&entry.control));
  }
  return NULL;
}
static void *wrong_owner(void *raw) {
  rinzu_callback_pointer_u64_entry_t *entry = raw;
  (void)rinzu_callback_entry_retire(&entry->control);
  return NULL;
}
static int compare(const void *left, const void *right) {
  uintptr_t a = *(const uintptr_t *)left, b = *(const uintptr_t *)right;
  return (a > b) - (a < b);
}
int main(int argc, char **argv) {
  uint64_t context = 11;
  unsigned char input = 3;
  if (argc == 2 && !strcmp(argv[1], "wrong-owner")) {
    rinzu_callback_pointer_u64_entry_t entry = rinzu_callback_pointer_u64_prepare(body, &context);
    assert(rinzu_callback_entry_close(entry.control));
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, wrong_owner, &entry));
    assert(!pthread_join(thread, NULL));
    return 90;
  }
  if (argc == 2 && !strcmp(argv[1], "null-owner")) {
    (void)rinzu_callback_entry_retire(NULL);
    return 90;
  }
  uintptr_t *addresses = calloc(ROUNDS + WORKERS * PER_WORKER, sizeof(*addresses));
  assert(addresses);
  size_t unknown = 0;
  for (size_t index = 0; index < ROUNDS; ++index) {
    context = index;
    rinzu_callback_pointer_u64_entry_t entry = rinzu_callback_pointer_u64_prepare(body, &context);
    addresses[index] = (uintptr_t)entry.entry;
    assert(!rinzu_callback_entry_retire(&entry.control));
    assert(entry.entry(&input) == index + input);
    assert(rinzu_callback_entry_close(entry.control));
    if (index % 16 == 15) {
      /* Unknown capability proof: deliberately keep the full tombstone. */
      unknown++;
    } else {
      assert(rinzu_callback_entry_retire(&entry.control));
      assert(!entry.control && rinzu_callback_entry_retire(&entry.control));
    }
  }
  rinzu_callback_pointer_u64_entry_t active = {0};
  active = rinzu_callback_pointer_u64_prepare(closing, &active);
  assert(active.entry(&input) == input && rinzu_callback_entry_settled(active.control));
  assert(rinzu_callback_entry_retire(&active.control));
  pthread_t threads[WORKERS];
  for (size_t index = 0; index < WORKERS; ++index)
    assert(!pthread_create(&threads[index], NULL, worker, addresses + ROUNDS + index * PER_WORKER));
  for (size_t index = 0; index < WORKERS; ++index)
    assert(!pthread_join(threads[index], NULL));
  if (argc == 2 && !strncmp(argv[1], "late-", 5)) {
    size_t index = !strcmp(argv[1], "late-first")    ? 0
                   : !strcmp(argv[1], "late-middle") ? ROUNDS / 2
                   : !strcmp(argv[1], "late-last")   ? ROUNDS - 2
                                                     : ROUNDS - 1;
    (void)((rinzu_callback_pointer_u64_t)addresses[index])(&input);
    return 90;
  }
  qsort(addresses, ROUNDS + WORKERS * PER_WORKER, sizeof(*addresses), compare);
  for (size_t index = 1; index < ROUNDS + WORKERS * PER_WORKER; ++index)
    assert(addresses[index - 1] != addresses[index]);
  rinzu_callback_stats_t stats = rinzu_callback_entry_stats();
  assert(stats.entries == ROUNDS + WORKERS * PER_WORKER + 1);
  assert(stats.closed_entries == stats.entries);
  assert(stats.retired_entries == stats.entries - unknown);
  assert(stats.retired_slot_bytes == stats.retired_entries * 32);
  size_t control_size = stats.released_control_bytes / stats.retired_entries;
  assert(stats.released_control_bytes == stats.retired_entries * control_size);
  assert(stats.tombstone_bytes == unknown * control_size);
  assert(stats.control_bytes == stats.blocks * 16384 + stats.tombstone_bytes);
  assert(!stats.live_connections && !stats.in_flight && !stats.live_diagnostics);
  printf(
      "PASS callback retirement entries=%zu retired=%zu unknown=%zu freed=%zu rw=%zu virtual=%zu\n",
      stats.entries, stats.retired_entries, unknown, stats.released_control_bytes,
      stats.control_bytes - stats.tombstone_bytes, stats.virtual_bytes);
  free(addresses);
}
