#include "rinzu/managed_composite.h"
#include "rinzu/native_opaque.h"
#include <assert.h>
#include <sys/wait.h>
#include <unistd.h>

static void publish(rinzu_managed_data_t *data, rinzu_managed_roots_t *roots, uint64_t handle) {
  rinzu_managed_publish(roots, &handle, 1);
  rinzu_managed_safepoint(data, roots);
}

int main(void) {
  rinzu_managed_data_t data = {0};
  rinzu_managed_roots_t active = {0}, parked = {0};
  rinzu_managed_root_set_t parked_set = {0};
  rinzu_managed_enter(&active);
  rinzu_managed_enter(&parked);
  rinzu_managed_attach_roots(&data, &parked_set, &parked);
  data.gc.stress = 0;
  uint64_t retained = rinzu_managed_string(&data, (const unsigned char *)"parked", 6);
  rinzu_managed_publish(&parked, &retained, 1);
  uint64_t anchor = rinzu_managed_string(&data, (const unsigned char *)"anchor", 6);
  publish(&data, &active, anchor);
  assert(data.collections == 0);
  for (int i = 0; i < 4096; ++i) {
    (void)rinzu_managed_string(&data, (const unsigned char *)"temporary", 9);
    publish(&data, &active, anchor);
    assert(data.heap_bytes < 2 * RINZU_GC_MIN_BUDGET);
  }
  assert(data.collections > 0 && data.collections < 16);
  assert(!memcmp(rinzu_managed_find(&data, retained)->bytes, "parked", 6));
  assert(!memcmp(rinzu_managed_find(&data, anchor)->bytes, "anchor", 6));
  uint64_t before = data.collections;
  for (int i = 0; i < 10000; ++i)
    publish(&data, &active, anchor);
  assert(data.collections == before);

  /* Large payloads charge their bytes, even with resource reporting disabled. */
  size_t length = 2 * RINZU_GC_MIN_BUDGET;
  unsigned char *payload = malloc(length);
  assert(payload);
  memset(payload, 'b', length);
  uint64_t large;
  assert(!rinzu_managed_bytes(&data, length, payload, length, &large));
  free(payload);
  assert(rinzu_gc_due(&data.gc));
  publish(&data, &active, large);
  assert(data.collections == before + 1);
  assert(data.gc.budget >= length);
  assert(rinzu_managed_find(&data, large)->bytes[length - 1] == 'b');
  assert(rinzu_managed_find(&data, retained)->length == 6);

  /* Explicit rollback/discard must remove live bytes but not refund pressure. */
  size_t heap_before = data.heap_bytes;
  rinzu_native_opaque_reservation_t reservation = rinzu_managed_reserve_opaque(&data, 0);
  assert(data.heap_bytes == heap_before + sizeof(rinzu_managed_value_t));
  rinzu_managed_discard_opaque(&data, &reservation);
  assert(data.heap_bytes == heap_before);

  /* Crossing a safepoint with a borrowed view remains an error even if GC is
     not due. Likewise a foreign handle is checked without waiting for GC. */
  for (int kind = 0; kind < 2; ++kind) {
    pid_t child = fork();
    assert(child >= 0);
    if (!child) {
      if (kind == 0)
        data.live_views = 1;
      else {
        rinzu_managed_data_t other = {0};
        large = rinzu_managed_string(&other, (const unsigned char *)"foreign", 7);
      }
      publish(&data, &active, large);
      _Exit(1);
    }
    int status;
    assert(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 70);
  }

  data.gc.stress = 1;
  for (int i = 0; i < 4; ++i) {
    uintptr_t address = (uintptr_t)rinzu_managed_find(&data, large)->bytes;
    publish(&data, &active, large);
    assert((uintptr_t)rinzu_managed_find(&data, large)->bytes != address);
  }
  assert(data.collections == before + 5);
  rinzu_managed_leave(&active);
  rinzu_managed_leave(&parked);
  rinzu_managed_detach_roots(&data, &parked_set);
  rinzu_managed_collect(&data, &active);
  assert(!data.objects && !data.heap_bytes && !data.index_capacity);
  rinzu_managed_data_dispose(&data);
  rinzu_managed_dispose(&active);
  rinzu_managed_dispose(&parked);
  puts("pacing: allocation bytes, parked roots, bounded heap, native views, foreign roots, stress, "
       "cleanup");
}
