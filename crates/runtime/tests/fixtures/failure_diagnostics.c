#include <stdlib.h>
#include <string.h>
static int fail_allocation;
static void *diagnostic_allocate(size_t count, size_t bytes) {
  return fail_allocation ? NULL : calloc(count, bytes);
}
#define calloc diagnostic_allocate
#include "rinzu/failure.h"
#undef calloc
#include <assert.h>

int main(int argc, char **argv) {
  rinzu_failure_state_t state = {0};
  rinzu_failure_print(&state);
  rinzu_failure_fail(&state, 5, (rinzu_source_position_t){1, 10, 11, 1});
  rinzu_failure_begin(&state);
  rinzu_failure_fail(&state, 12, (rinzu_source_position_t){1, 20, 21, 1});
  rinzu_failure_end(&state);
  assert(rinzu_failure_observe(&state) == 1);
  rinzu_failure_fail(&state, 15, (rinzu_source_position_t){2, 30, 31, 1});
  assert(rinzu_failure_observe(&state) == 2);
  rinzu_observed_error_t *head = state.observed, *tail = head->next;
  if (argc > 1) {
    if (!strcmp(argv[1], "missing"))
      state.observed_count = 3;
    else if (!strcmp(argv[1], "extra"))
      state.observed_count = 1;
    else if (!strcmp(argv[1], "stray"))
      state.observed_count = 0;
    else if (!strcmp(argv[1], "duplicate"))
      tail->identity = 2;
    else if (!strcmp(argv[1], "order")) {
      head->identity = 1;
      tail->identity = 2;
    } else if (!strcmp(argv[1], "overflow"))
      state.observed_count = SIZE_MAX / sizeof(void *) + 1;
    else if (!strcmp(argv[1], "oom"))
      fail_allocation = 1;
    else {
      rinzu_failure_dispose(&state);
      return 2;
    }
    rinzu_failure_print(&state);
    rinzu_failure_dispose(&state);
    return 3;
  }
  for (int pass = 0; pass < 2; ++pass) {
    rinzu_failure_print(&state);
    assert(state.observed == head && head->next == tail && !tail->next);
    assert(head->identity == 2 && tail->identity == 1 && state.observed_count == 2);
    assert(tail->failure.head && tail->failure.head->cause == 12);
  }
  rinzu_failure_dispose(&state);
  assert(!state.observed && !state.observed_count);
  return 0;
}
