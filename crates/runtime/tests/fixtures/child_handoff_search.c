#include <assert.h>
#include <stdlib.h>
#include <string.h>
static int fail_allocations;
static void *test_calloc(size_t count, size_t size) {
  return fail_allocations ? NULL : calloc(count, size);
}
#define calloc test_calloc
#include "rinzu/child_scope.h"
#undef calloc

static void ordered(rinzu_child_scope_t *scope, rinzu_child_t **expected, unsigned count) {
  rinzu_child_t *previous = NULL, *child = scope->children;
  for (unsigned i = 0; i < count; ++i) {
    assert(child == expected[i]);
    assert(child->previous == previous);
    assert(!previous || previous->identity < child->identity);
    assert(rinzu_child_find(scope, child->identity) == child);
    previous = child;
    child = child->next;
  }
  assert(!child && scope->children_tail == previous && scope->child_count == count);
  child = scope->children_tail;
  for (unsigned i = count; i; --i) {
    assert(child == expected[i - 1]);
    child = child->previous;
  }
  assert(!child);
}
static void finish(rinzu_child_scope_t *scope) {
  for (rinzu_child_t *child = scope->children; child; child = child->next) {
    child->completed = child->observed = 1;
  }
  if (!scope->closing) assert(!rinzu_child_begin_close(scope, NULL));
  assert(!rinzu_child_finish_close(scope));
  ordered(scope, NULL, 0);
  rinzu_child_dispose(scope);
}
static void *foreign(void *raw) {
  rinzu_child_scope_t *scope = raw;
  (void)rinzu_child_handoff(scope, scope + 1);
  return NULL;
}
int main(int argc, char **argv) {
  if (argc > 1) {
    rinzu_child_scope_t scopes[2];
    rinzu_child_init(&scopes[0]); rinzu_child_init(&scopes[1]);
    if (!strcmp(argv[1], "owner")) {
      pthread_t thread;
      assert(!pthread_create(&thread, NULL, foreign, scopes));
      assert(!pthread_join(thread, NULL));
    } else if (!strcmp(argv[1], "oom")) {
      assert(!rinzu_child_admit(&scopes[0], 1));
      for (uint64_t i = 0; i < 8; ++i) assert(!rinzu_child_admit(&scopes[1], i + 10));
      fail_allocations = 1;
      (void)rinzu_child_handoff(&scopes[0], &scopes[1]);
    } else {
      unsigned position = (unsigned)strtoul(argv[1], NULL, 10);
      assert(position < 3);
      for (uint64_t i = 0; i < 3; ++i) assert(!rinzu_child_admit(&scopes[1], i));
      assert(!rinzu_child_admit(&scopes[0], position));
      (void)rinzu_child_handoff(&scopes[0], &scopes[1]);
    }
    assert(0 && "must reject foreign Owner, duplicate or allocation failure");
  }
  /* Every disjoint partition of ten sorted identities, including absent IDs,
     empty lists, interleaving, either edge and UINT64_MAX. Admission is permuted. */
  for (unsigned pattern = 0; pattern < 59049; ++pattern) {
    rinzu_child_scope_t source, destination;
    rinzu_child_init(&source); rinzu_child_init(&destination);
    rinzu_child_t *by_id[10] = {0}, *expected[10];
    unsigned membership[10], code = pattern, count = 0;
    rinzu_error_reference_t error = {.references = 1, .owner = &source};
    for (unsigned i = 0; i < 10; ++i) { membership[i] = code % 3; code /= 3; }
    for (unsigned n = 0; n < 10; ++n) {
      unsigned i = (n * 7) % 10;
      if (!membership[i]) continue;
      uint64_t id = i == 9 ? UINT64_MAX : i * 7;
      rinzu_child_scope_t *scope = membership[i] == 1 ? &source : &destination;
      assert(!rinzu_child_admit(scope, id));
      rinzu_child_t *child = by_id[i] = rinzu_child_find(scope, id);
      child->completed = i & 1; child->observed = (i >> 1) & 1;
      child->expected_control = (i >> 2) & 1;
      child->failure.primary = 5;
      child->failure.primary_source = (rinzu_source_position_t){.file=i, .start_byte=i+1, .end_byte=i+2, .present=1};
      child->failure.primary_error = rinzu_error_reference_copy(&error);
    }
    for (unsigned i = 0; i < 10; ++i) if (by_id[i]) expected[count++] = by_id[i];
    size_t references = error.references;
    /* A draining supervisor is explicitly permitted by the contract. */
    destination.closing = pattern & 1;
    assert(!rinzu_child_handoff(&source, &destination));
    ordered(&source, NULL, 0); ordered(&destination, expected, count);
    assert(error.references == references);
    for (unsigned i = 0; i < 10; ++i) if (by_id[i]) {
      rinzu_child_t *child = by_id[i];
      assert(child->completed == (int)(i & 1) && child->observed == (int)((i >> 1) & 1));
      assert(child->expected_control == (int)((i >> 2) & 1));
      assert(child->failure.primary == 5 && child->failure.primary_error == &error);
      assert(child->failure.primary_source.file == i);
    }
    finish(&source); finish(&destination);
    assert(error.references == 1);
  }
  for (unsigned reason = 0; reason < 4; ++reason) {
    rinzu_child_scope_t a, b;
    rinzu_child_init(&a); rinzu_child_init(&b);
    assert(!rinzu_child_admit(&a, 1)); assert(!rinzu_child_admit(&b, 2));
    rinzu_child_t *one[] = {a.children}, *two[] = {b.children};
    if (reason == 0) assert(rinzu_child_handoff(&a, &a) == RINZU_CHILD_CLOSED);
    else {
      if (reason == 1) a.closing = 1;
      if (reason == 2) a.finished = 1;
      if (reason == 3) b.finished = 1;
      assert(rinzu_child_handoff(&a, &b) == RINZU_CHILD_CLOSED);
    }
    ordered(&a, one, 1); ordered(&b, two, 1);
    a.finished = b.finished = 0;
    finish(&a); finish(&b);
  }
  return 0;
}
