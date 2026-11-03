#include "rinzu/owner_transition.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>

static void nested(rinzu_owner_transition_t *owner, unsigned depth) {
  unsigned state = owner->state;
  uint64_t map = owner->stack_map;
  size_t roots = owner->roots;
  rinzu_owner_native_frame_t frame = {0};
  rinzu_owner_native_enter(owner, &frame, depth + 1, depth + 3);
  assert(owner->state == RINZU_OWNER_NATIVE_SAFE && owner->stack_map == depth + 1 &&
         owner->roots == depth + 3);
  rinzu_owner_callback_scope_t scope = rinzu_owner_callback_enter(owner);
  assert(owner->state == RINZU_OWNER_CALLBACK_MANAGED);
  if (depth)
    nested(owner, depth - 1);
  rinzu_owner_callback_leave(scope);
  assert(owner->state == RINZU_OWNER_NATIVE_SAFE && owner->current == &frame);
  rinzu_owner_native_leave(owner, &frame);
  assert(owner->state == state && owner->stack_map == map && owner->roots == roots &&
         !frame.active);
}
static void *worker(void *raw) {
  (void)raw;
  rinzu_owner_transition_t owner = {0};
  rinzu_owner_transition_init(&owner);
  nested(&owner, 63);
  rinzu_owner_transition_finish(&owner);
  assert(owner.state == RINZU_OWNER_INACTIVE && !owner.current && !owner.roots);
  return NULL;
}
static void *wrong_thread(void *raw) {
  rinzu_owner_transition_managed(raw);
  return NULL;
}
int main(int argc, char **argv) {
  rinzu_owner_transition_t owner = {0};
  rinzu_owner_transition_init(&owner);
  if (argc > 1) {
    rinzu_owner_native_frame_t outer = {0}, inner = {0};
    if (!strcmp(argv[1], "thread")) {
      pthread_t thread;
      assert(!pthread_create(&thread, NULL, wrong_thread, &owner));
      assert(!pthread_join(thread, NULL));
    } else if (!strcmp(argv[1], "closed")) {
      rinzu_owner_transition_finish(&owner);
      rinzu_owner_transition_init(&owner);
    } else if (!strcmp(argv[1], "managed-callback"))
      rinzu_owner_callback_enter(&owner);
    else if (!strcmp(argv[1], "overflow")) {
      owner.generation = UINT64_MAX;
      rinzu_owner_native_enter(&owner, &outer, 1, 1);
    } else {
      rinzu_owner_native_enter(&owner, &outer, 1, 1);
      rinzu_owner_callback_scope_t scope = rinzu_owner_callback_enter(&owner);
      rinzu_owner_native_enter(&owner, &inner, 2, 2);
      if (!strcmp(argv[1], "order"))
        rinzu_owner_native_leave(&owner, &outer);
      else if (!strcmp(argv[1], "stale")) {
        rinzu_owner_native_leave(&owner, &inner);
        rinzu_owner_native_leave(&owner, &inner);
      } else if (!strcmp(argv[1], "callback-order"))
        rinzu_owner_callback_leave(scope);
      else if (!strcmp(argv[1], "finish"))
        rinzu_owner_transition_finish(&owner);
      else
        return 2;
    }
    return 3;
  }
  nested(&owner, 63);
  rinzu_owner_transition_finish(&owner);
  pthread_t threads[8];
  for (unsigned i = 0; i < 8; i++)
    assert(!pthread_create(&threads[i], NULL, worker, NULL));
  for (unsigned i = 0; i < 8; i++)
    assert(!pthread_join(threads[i], NULL));
  puts("PASS Owner transitions 576 nested levels");
  return 0;
}
