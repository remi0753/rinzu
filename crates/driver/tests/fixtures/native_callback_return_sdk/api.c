#include "api.h"
#include <stdlib.h>
#include <string.h>
#include <assert.h>
#include <pthread.h>
#include <unistd.h>
static _Thread_local size_t ticks;
typedef struct Held {
  const unsigned char *data;
  size_t length;
  unsigned char expected[18];
  struct Held *next;
} Held;
static Held *held;
static pthread_mutex_t mutex = PTHREAD_MUTEX_INITIALIZER;
static void keep(const unsigned char *data, const unsigned char *expected, size_t length) {
  assert(data && length <= 18 && !memcmp(data, expected, length));
  Held *node = malloc(sizeof(*node));
  if (!node)
    _Exit(71);
  node->data = data;
  node->length = length;
  memcpy(node->expected, expected, length);
  unsigned char *owned = malloc(length);
  if (!owned)
    _Exit(71);
  memcpy(owned, data, length);
  memset(owned, 42, length);
  free(owned);
  assert(!memcmp(data, expected, length));
  assert(!pthread_mutex_lock(&mutex));
  node->next = held;
  held = node;
  assert(!pthread_mutex_unlock(&mutex));
}
void return_tick(void) {
  ticks++;
}
uint64_t return_strings(const char *(*callback)(int32_t, const char *), int32_t mode) {
  const unsigned char normal[] = "日本語😀\0end", fallback[] = "代替\0tail";
  size_t before = ticks;
  for (int i = 0; i < 4; i++) {
    const unsigned char *result =
        (const unsigned char *)callback(mode, mode == 2 ? NULL : "input\0ignored");
    if (mode == 0 || mode == 4)
      keep(result, normal, sizeof(normal));
    else
      keep(result, fallback, sizeof(fallback));
  }
  assert(ticks - before == (mode == 2 ? 0 : 4));
  return 4;
}
uint64_t return_bytes(const uint8_t *(*callback)(int32_t, const char *), int32_t mode) {
  const unsigned char normal[] = {255, 0, 7, 0}, fallback[] = {254, 0, 0, 0};
  size_t before = ticks;
  for (int i = 0; i < 4; i++)
    keep(callback(mode, mode == 2 ? NULL : "input"), (mode == 0 || mode == 4) ? normal : fallback,
         4);
  assert(ticks - before == (mode == 2 ? 0 : 4));
  return 4;
}
int32_t return_zero(const uint8_t *(*callback)(int32_t)) {
  const unsigned char expected[] = {0};
  size_t before = ticks;
  for (int mode = 0; mode < 2; mode++)
    for (int i = 0; i < 4; i++)
      keep(callback(mode), expected, 1);
  assert(ticks - before == 8);
  return 8;
}
int32_t return_empty(const char *(*callback)(int32_t)) {
  const unsigned char expected[] = {0};
  size_t before = ticks;
  for (int mode = 0; mode < 2; mode++)
    for (int i = 0; i < 4; i++)
      keep((const unsigned char *)callback(mode), expected, 1);
  assert(ticks - before == 8);
  return 8;
}
int32_t return_unit(void (*callback)(int32_t)) {
  size_t before = ticks;
  for (int mode = 0; mode < 2; mode++)
    for (int i = 0; i < 4; i++)
      callback(mode);
  assert(ticks - before == 8);
  return 8;
}
int32_t return_held(void) {
  assert(!pthread_mutex_lock(&mutex));
  Held *values = held;
  held = NULL;
  assert(!pthread_mutex_unlock(&mutex));
  while (values) {
    Held *next = values->next;
    assert(!memcmp(values->data, values->expected, values->length));
    free(values);
    values = next;
  }
  return 1;
}
