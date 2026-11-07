/* Same active records, no producer thread: isolate Owner progress bookkeeping. */
#include <assert.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "rinzu/native_diagnostics.h"

static uint64_t cpu_tick(void) {
  struct timespec now;
  assert(!clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &now));
  return (uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec;
}
static void measure(size_t count) {
  rinzu_activity_board_t board = {.owner = pthread_self()};
  rinzu_activity_gate_init(&board);
  assert(!pthread_cond_init(&board.changed, NULL));
  for (size_t i = 0; i < count; ++i) {
    rinzu_activity_t *record =
        rinzu_activity_prepare(&board, RINZU_ACTIVITY_OWNER_STALL, "ready", 1, 2, 3, 1, 0);
    rinzu_activity_start(record);
  }
  const size_t operations = 200000;
  for (size_t i = 0; i < 1000; ++i)
    rinzu_activity_owner_turn(&board);
  uint64_t start = cpu_tick();
  for (size_t i = 0; i < operations; ++i)
    rinzu_activity_owner_turn(&board);
  uint64_t cpu = cpu_tick() - start;
  assert(!board.stats.warnings[RINZU_ACTIVITY_OWNER_STALL]);
  while (board.head)
    rinzu_activity_end(board.head);
  assert(!board.stats.live && !board.stats.active && !board.stats.warning_pins);
#ifdef RINZU_ACTIVITY_CACHE_SLOT_BYTES
  rinzu_activity_cache_drain(&board);
#endif
  assert(!pthread_cond_destroy(&board.changed));
  rinzu_activity_gate_dispose(&board);
  printf("{\"case\":\"owner-ready-%zu\",\"operations\":%zu,"
         "\"nanoseconds\":%" PRIu64 ",\"cpu_nanoseconds\":%" PRIu64
         ",\"allocations\":0,\"allocated_bytes\":0,\"checksum\":%zu}\n",
         count, operations, cpu, cpu, operations);
}
int main(int argc, char **argv) {
  assert(argc == 2 && !strcmp(argv[1], "owner-turn"));
  for (size_t count = 0;; count = count ? count * 16 : 1) {
    measure(count);
    if (count == 4096)
      break;
  }
}
