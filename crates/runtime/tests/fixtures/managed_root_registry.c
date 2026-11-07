#include <assert.h>
#include <string.h>
#include "rinzu/managed_data.h"

static void counts(rinzu_managed_data_t *heap, rinzu_managed_roots_t *current) {
  size_t slow = 0;
  for (rinzu_managed_root_set_t *entry = heap->root_sets; entry; entry = entry->next)
    slow += rinzu_managed_frame_chain_count(entry->roots);
  assert(slow == heap->published_root_count);
  if (!current->domain_entry)
    slow += rinzu_managed_frame_chain_count(current);
  assert(slow == rinzu_managed_native_root_count(heap, current));
}

int main(int argc, char **argv) {
  rinzu_managed_data_t heaps[2] = {0};
  enum { COUNT = 4096 };
  rinzu_managed_roots_t *roots = calloc(COUNT, sizeof(*roots));
  rinzu_managed_root_set_t *entries = calloc(COUNT, sizeof(*entries));
  assert(roots && entries);
  for (unsigned batch = 0; batch < 2; ++batch) {
    rinzu_managed_data_t *heap = &heaps[batch];
    for (unsigned i = 0; i < COUNT; ++i) {
      rinzu_managed_attach_roots(heap, &entries[i], &roots[i]);
      assert(roots[i].domain_entry == &entries[i]);
      assert(entries[i].next == (i ? &entries[i - 1] : NULL));
      assert(!entries[i].previous);
    }
    if (argc > 1) {
      rinzu_managed_root_set_t duplicate = {0};
      if (!strcmp(argv[1], "same-roots"))
        rinzu_managed_attach_roots(heap, &duplicate, &roots[0]);
      else if (!strcmp(argv[1], "other-domain"))
        rinzu_managed_attach_roots(&heaps[1], &duplicate, &roots[0]);
      else if (!strcmp(argv[1], "attached-dispose"))
        rinzu_managed_dispose(&roots[0]);
      else if (!strcmp(argv[1], "count-wrong-domain"))
        rinzu_managed_native_root_count(&heaps[1], &roots[0]);
      else if (!strcmp(argv[1], "count-overflow")) {
        uint64_t value = 1;
        heap->published_root_count = SIZE_MAX;
        rinzu_managed_enter(&roots[0]);
        rinzu_managed_publish(&roots[0], &value, 1);
      } else if (!strcmp(argv[1], "count-underflow")) {
        uint64_t value = 1;
        rinzu_managed_enter(&roots[0]);
        rinzu_managed_publish(&roots[0], &value, 1);
        heap->published_root_count = 0;
        rinzu_managed_leave(&roots[0]);
      } else if (!strcmp(argv[1], "wrong-entry")) {
        duplicate = entries[0];
        rinzu_managed_detach_roots(heap, &duplicate);
      } else if (!strcmp(argv[1], "broken-link")) {
        entries[0].previous = &entries[2];
        rinzu_managed_detach_roots(heap, &entries[0]);
      } else
        return 2;
      return 3;
    }
    uint64_t value = rinzu_managed_string(heap, (const unsigned char *)"kept", 4);
    uint64_t words[] = {value, value, value};
    for (unsigned i = 0; i < COUNT; ++i) {
      rinzu_managed_enter(&roots[i]);
      rinzu_managed_publish(&roots[i], words, i % 4);
      rinzu_managed_enter(&roots[i]);
      size_t before = heap->published_root_count;
      uint64_t *prepared = rinzu_managed_prepare_roots(&roots[i], i % 3);
      for (unsigned n = 0; n < i % 3; ++n)
        prepared[n] = value;
      assert(heap->published_root_count == before);
      rinzu_managed_commit_roots(&roots[i], i % 3);
      if (!(i % 256))
        counts(heap, &roots[i]);
    }
    rinzu_managed_collect(heap, &roots[0]);
    assert(heap->objects == 1);
    counts(heap, &roots[0]);
    for (unsigned i = 0; i < COUNT; ++i) {
      unsigned slot = (i * 661) & (COUNT - 1);
      rinzu_managed_leave(&roots[slot]);
      rinzu_managed_publish(&roots[slot], NULL, 0);
      rinzu_managed_leave(&roots[slot]);
      rinzu_managed_detach_roots(heap, &entries[slot]);
      assert(!roots[slot].domain_entry && !roots[slot].domain_published_count &&
             !entries[slot].data && !entries[slot].roots && !entries[slot].next &&
             !entries[slot].previous);
      if (!(i % 256)) {
        size_t count = 0;
        rinzu_managed_root_set_t *previous = NULL;
        for (rinzu_managed_root_set_t *entry = heap->root_sets; entry; entry = entry->next) {
          assert(entry->previous == previous && entry->data == heap &&
                 entry->roots->domain_entry == entry);
          previous = entry;
          ++count;
        }
        assert(count == COUNT - i - 1);
        counts(heap, &roots[slot]);
      }
    }
    assert(!heap->root_sets);
    assert(!heap->published_root_count);
    rinzu_managed_roots_t detached = {0};
    rinzu_managed_enter(&detached);
    rinzu_managed_publish(&detached, words, 2);
    counts(heap, &detached);
    rinzu_managed_leave(&detached);
    rinzu_managed_dispose(&detached);
    rinzu_managed_collect(heap, &roots[0]);
    assert(!heap->objects);
  }
  for (unsigned i = 0; i < COUNT; ++i)
    rinzu_managed_dispose(&roots[i]);
  rinzu_managed_data_dispose(&heaps[0]);
  rinzu_managed_data_dispose(&heaps[1]);
  free(roots);
  free(entries);
  return 0;
}
