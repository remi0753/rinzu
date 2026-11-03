#include "api.h"
#include <assert.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <errno.h>
static pthread_mutex_t gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t changed = PTHREAD_COND_INITIALIZER;
static pthread_t owner;
static int entered, stopped, returned, delayed, origin_cleanup, worker_cleanup;
static void mark(const char *name) {
  fprintf(stderr, "PUMP_%s\n", name);
}
static void input(const char *text, const uint8_t *bytes) {
  const uint8_t expected[4] = {0, 170, 0, 127};
  assert(text && !strcmp(text, "blocker 日本語") && bytes && !memcmp(bytes, expected, 4));
}
static void same_owner(void) {
  assert(pthread_equal(owner, pthread_self()));
}
void pump_reset(void) {
  alarm(45);
  owner = pthread_self();
  assert(!pthread_mutex_lock(&gate));
  assert(!entered || (stopped && returned && delayed && origin_cleanup && worker_cleanup));
  entered = stopped = returned = delayed = origin_cleanup = worker_cleanup = 0;
  assert(!pthread_mutex_unlock(&gate));
  mark("RESET");
}
int32_t pump_block(const char *text, const uint8_t *bytes) {
  assert(!pthread_equal(owner, pthread_self()));
  input(text, bytes);
  assert(!pthread_mutex_lock(&gate));
  assert(!entered);
  entered = 1;
  mark("NATIVE_ENTER");
  assert(!pthread_cond_broadcast(&changed));
  while (!stopped)
    assert(!pthread_cond_wait(&changed, &gate));
  input(text, bytes);
  assert(delayed && !origin_cleanup);
  returned = 1;
  mark("NATIVE_RETURN");
  assert(!pthread_mutex_unlock(&gate));
  return 41;
}
void pump_wait_entries(void) {
  same_owner();
  assert(!pthread_mutex_lock(&gate));
  while (!entered)
    assert(!pthread_cond_wait(&changed, &gate));
  assert(!pthread_mutex_unlock(&gate));
  mark("ENTRIES_OBSERVED");
}
int32_t pump_delay(void) {
  assert(!pthread_equal(owner, pthread_self()));
  mark("DELAY_ENTER");
  struct timespec wait = {6, 0};
  while (nanosleep(&wait, &wait))
    assert(errno == EINTR);
  assert(!pthread_mutex_lock(&gate));
  assert(entered && !origin_cleanup && !stopped && !returned);
  delayed = 1;
  mark("DELAY_RETURN");
  assert(!pthread_mutex_unlock(&gate));
  return 13;
}
void pump_stop(void) {
  same_owner();
  assert(!pthread_mutex_lock(&gate));
  assert(entered && delayed && !origin_cleanup && !stopped && !returned);
  stopped = 1;
  mark("MAIN_STOP");
  assert(!pthread_cond_broadcast(&changed));
  assert(!pthread_mutex_unlock(&gate));
}
void pump_origin_finally(void) {
  same_owner();
  assert(!pthread_mutex_lock(&gate));
  assert(entered && stopped && returned && worker_cleanup && !origin_cleanup);
  origin_cleanup = 1;
  mark("ORIGIN_FINALLY");
  assert(!pthread_mutex_unlock(&gate));
}
void pump_worker_finally(const char *text, const uint8_t *bytes) {
  assert(!pthread_equal(owner, pthread_self()));
  input(text, bytes);
  assert(!pthread_mutex_lock(&gate));
  assert(returned && !worker_cleanup);
  worker_cleanup = 1;
  mark("WORKER_FINALLY");
  assert(!pthread_mutex_unlock(&gate));
}
