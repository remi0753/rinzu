#include <assert.h>
#include "rinzu/child_scope.h"

static void ordered(rinzu_child_scope_t *scope, uint64_t count) {
  rinzu_child_t *last = NULL;
  uint64_t seen = 0;
  for (rinzu_child_t *child = scope->children; child; child = child->next) {
    assert(!last || last->identity < child->identity);
    assert(child->previous == last);
    assert(rinzu_child_find(scope, child->identity) == child);
    last = child;
    ++seen;
  }
  assert(seen == count && scope->child_count == count && scope->children_tail == last);
}
static void finish(rinzu_child_scope_t *scope) {
  for (rinzu_child_t *child = scope->children; child; child = child->next)
    assert(!rinzu_child_complete(scope, child->identity, NULL));
  assert(!rinzu_child_begin_close(scope, NULL));
  assert(!rinzu_child_finish_close(scope));
  ordered(scope, 0);
  rinzu_child_dispose(scope);
}
int main(void) {
  for (int all = 0; all < 2; ++all) {
    rinzu_child_scope_t a, b;
    rinzu_child_init(&a);
    if (all)
      rinzu_child_init_all(&b);
    else
      rinzu_child_init(&b);
    /* Odd multiplication permutes all 1024 identities. */
    for (uint64_t i = 0; i < 1024; ++i) {
      uint64_t id = (i * 661) & 1023;
      assert(!rinzu_child_admit(id & 1 ? &a : &b, id));
    }
    ordered(&a, 512);
    ordered(&b, 512);
    assert(!rinzu_child_handoff(&a, &b));
    ordered(&a, 0);
    ordered(&b, 1024);
    assert(rinzu_child_admit(&b, 1023) == RINZU_CHILD_DUPLICATE);
    assert(!rinzu_child_withdraw_admission(&b, 1023));
    assert(!rinzu_child_admit(&b, 1024));
    assert(!rinzu_child_complete(&b, 1024, NULL));
    assert(!rinzu_child_observe(&b, 1024));
    assert(!rinzu_child_retire_observed(&b, 1024));
    assert(!rinzu_child_admit(&b, 1025));
    assert(!rinzu_child_complete(&b, 1025, NULL));
    assert(rinzu_child_retire_unreferenced(&b, 1025));
    ordered(&b, 1023);
    /* Remove/reinsert the sole record; a stale tail would be freed storage. */
    assert(!rinzu_child_admit(&a, UINT64_MAX));
    assert(!rinzu_child_withdraw_admission(&a, UINT64_MAX));
    ordered(&a, 0);
    assert(!rinzu_child_admit(&a, 2048));
    assert(!rinzu_child_handoff(&a, &b));
    ordered(&b, 1024);
    ordered(&a, 0);
    finish(&a);
    finish(&b);
  }
  return 0;
}
