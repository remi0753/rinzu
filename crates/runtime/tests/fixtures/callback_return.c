#include <stdlib.h>
#include <stdint.h>
#include <assert.h>
#include <string.h>
#include <stdio.h>
#include <pthread.h>
static _Thread_local long budget = -1;
static int allowed(void) {
  if (!budget)
    return 0;
  if (budget > 0)
    --budget;
  return 1;
}
static void *allocate(size_t bytes) {
  return allowed() ? malloc(bytes) : NULL;
}
static void *zero(size_t count, size_t bytes) {
  return allowed() ? calloc(count, bytes) : NULL;
}
#define malloc allocate
#define calloc zero
#include "rinzu/callback_return.h"
typedef struct {
  size_t count;
  const unsigned char *first, *last, *fallback;
} Owner;
static void *owner(void *raw) {
  Owner *state = raw;
  for (size_t i = 0; i < state->count; ++i) {
    unsigned char text[] = "日本語\0x";
    const unsigned char *result =
        rinzu_callback_return_buffer_copy(text, sizeof(text) - 1, sizeof(text) - 1, 1, 0);
    if (!state->first)
      state->first = result;
    state->last = result;
    const unsigned char bytes[] = {0, 255, 7, 0};
    const unsigned char *copied = rinzu_callback_return_buffer_copy(bytes, 4, 4, 0, 0);
    assert(!memcmp(copied, bytes, 4));
    state->fallback = rinzu_callback_return_buffer_copy(bytes, 2, 8, 0, 1);
    memset(text, 42, sizeof(text));
  }
  return NULL;
}
int main(int argc, char **argv) {
  if (argc > 1) {
    const char *mode = argv[1];
    unsigned fallback = !strncmp(mode, "fallback", 8);
    if (strstr(mode, "node"))
      budget = 0;
    else if (strstr(mode, "buffer"))
      budget = 1;
    if (!strcmp(mode, "size"))
      rinzu_callback_return_buffer_copy(NULL, 0, SIZE_MAX, 0, 0);
    else if (!strcmp(mode, "prefix"))
      rinzu_callback_return_buffer_copy((const unsigned char *)"x", 1, 0, 0, 0);
    else if (!strcmp(mode, "null"))
      rinzu_callback_return_buffer_copy(NULL, 1, 1, 0, 0);
    else if (!strcmp(mode, "string-padding"))
      rinzu_callback_return_buffer_copy((const unsigned char *)"x", 1, 2, 1, 0);
    else if (!strcmp(mode, "kind"))
      rinzu_callback_return_buffer_copy(NULL, 0, 0, 2, 0);
    else
      rinzu_callback_return_buffer_copy((const unsigned char *)"x", 1, 1, 1, fallback);
    return 90;
  }
  Owner owners[9] = {{.count = 1031}};
  pthread_t workers[8];
  owner(&owners[0]);
  for (size_t i = 1; i < 9; ++i) {
    owners[i].count = 129;
    assert(!pthread_create(&workers[i - 1], NULL, owner, &owners[i]));
  }
  for (size_t i = 0; i < 8; ++i)
    assert(!pthread_join(workers[i], NULL));
  const unsigned char original[] = "日本語\0x";
  const unsigned char fallback[] = {0, 255, 0, 0, 0, 0, 0, 0};
  for (size_t i = 0; i < 9; ++i) {
    assert(!memcmp(owners[i].first, original, sizeof(original)));
    assert(!memcmp(owners[i].last, original, sizeof(original)));
    assert(!memcmp(owners[i].fallback, fallback, 8));
  }
  rinzu_callback_return_stats_t stats = rinzu_callback_return_buffer_stats();
  assert(stats.normal == 4126 && stats.fallback == 2063 && stats.bytes == 49512 &&
         stats.metadata_bytes == 6189 * sizeof(rinzu_callback_return_node_t));
  unsigned char *native = malloc(sizeof(original));
  assert(native);
  memcpy(native, owners[0].first, sizeof(original));
  memset(native, 42, sizeof(original));
  free(native);
  assert(!memcmp(owners[0].first, original, sizeof(original)));
  puts("PASS return arena 6189 buffers, Owner exit and native-owned copy");
  return 0;
}
