#include "api.h"
#include <assert.h>
#include <pthread.h>
#include <stdio.h>
static pthread_t owner;
static int attached, phase[2], finished;
static void on_owner(void) {
  if (!attached) {
    attached = 1;
    owner = pthread_self();
  }
  assert(pthread_equal(owner, pthread_self()));
}
int32_t capsule_invoke(int32_t (*body)(int32_t), int32_t identity) {
  on_owner();
  assert(body && identity >= 1 && identity <= 2);
  int32_t value = body(identity);
  on_owner();
  assert(phase[identity - 1] == 1);
  return value;
}
void capsule_mark(int32_t identity, int32_t step) {
  on_owner();
  assert(identity >= 1 && identity <= 2);
  int *state = &phase[identity - 1];
  if (step == 1) {
    assert(*state == 0);
    *state = 1;
  } else if (step == 3) {
    assert(*state == 1);
    *state = 3;
  } else if (step == 4) {
    assert(*state == 3);
    *state = 4;
  } else if (step == 2) {
    assert(*state == 4);
    *state = 2;
  } else if (step == 5) {
    assert(*state == 2);
    *state = 5;
  } else
    assert(0);
}
int32_t capsule_finish(int32_t process_main) {
  on_owner();
  if (process_main)
    assert(pthread_main_np());
  assert(phase[0] == 5 && phase[1] == 5);
  assert(!finished);
  finished = 1;
  return 2;
}
static pthread_t cancel_owner;
static int cancel_inner, cancel_returned, cancel_outer;
void capsule_cancel_reset(void) {
  cancel_owner = pthread_self();
  cancel_inner = cancel_returned = cancel_outer = 0;
}
static void cancel_on_owner(void) {
  assert(pthread_equal(cancel_owner, pthread_self()));
}
int32_t capsule_cancel_invoke(int32_t (*body)(int32_t)) {
  cancel_on_owner();
  assert(body);
  int32_t value = body(1);
  cancel_on_owner();
  assert(value == 41 && cancel_inner == 1 && !cancel_outer);
  cancel_returned = 1;
  fputs("CAPSULE_CANCEL_RESULT=41\n", stderr);
  return value;
}
void capsule_cancel_inner(void) {
  cancel_on_owner();
  assert(!cancel_inner && !cancel_returned && !cancel_outer);
  cancel_inner = 1;
  fputs("CAPSULE_CANCEL_FINALLY=1\n", stderr);
}
void capsule_cancel_outer(void) {
  cancel_on_owner();
  assert(cancel_inner && cancel_returned && !cancel_outer);
  cancel_outer = 1;
  fputs("CAPSULE_CANCEL_OUTER=1\n", stderr);
}
