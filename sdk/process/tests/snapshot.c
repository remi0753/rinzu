#define _POSIX_C_SOURCE 200809L
#include "../native/process.h"
#include <assert.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
static void *thread_snapshot(void *unused) {
  (void)unused;
  for (int i = 0; i < 1000; ++i) {
    assert(rinzu_process_environment_read("RINZU_PROCESS_TEST_VALUE") == 7);
    assert(memcmp(rinzu_process_environment_bytes(), "owned-1", 7) == 0);
  }
  return NULL;
}
int main(void) {
  assert(setenv("RINZU_PROCESS_TEST_VALUE", "owned-1", 1) == 0);
  assert(rinzu_process_environment_read("RINZU_PROCESS_TEST_MISSING") == -1);
  assert(rinzu_process_environment_read("bad=name") == -2);
  assert(rinzu_process_environment_read(NULL) == -2);
  assert(setenv("RINZU_PROCESS_TEST_EMPTY", "", 1) == 0);
  assert(rinzu_process_environment_read("RINZU_PROCESS_TEST_EMPTY") == 0);
  char limit[4098];
  memset(limit, 'x', sizeof limit);
  limit[4096] = 0;
  assert(setenv("RINZU_PROCESS_TEST_LIMIT", limit, 1) == 0);
  assert(rinzu_process_environment_read("RINZU_PROCESS_TEST_LIMIT") == 4096);
  assert(rinzu_process_environment_bytes()[4095] == 'x');
  limit[4096] = 'x';
  limit[4097] = 0;
  assert(setenv("RINZU_PROCESS_TEST_LIMIT", limit, 1) == 0);
  assert(rinzu_process_environment_read("RINZU_PROCESS_TEST_LIMIT") == -2);
  pthread_t thread;
  assert(pthread_create(&thread, NULL, thread_snapshot, NULL) == 0);
  for (int i = 0; i < 1000; ++i) {
    assert(rinzu_process_environment_read("RINZU_PROCESS_TEST_EMPTY") == 0);
    assert(rinzu_process_environment_bytes()[0] == 0);
  }
  assert(pthread_join(thread, NULL) == 0);
  puts("process-snapshot-ok");
}
