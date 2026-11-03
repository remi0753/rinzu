#include <pthread.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
static _Atomic uint64_t touches;
static _Thread_local uint64_t (*previous)(const uint8_t *);
uint64_t invoke(uint64_t (*callback)(const uint8_t *), const uint8_t *data) {
  pthread_t origin = pthread_self();
  if (!data || data[0] != 65 || data[1] != 255 || data[2] || data[3])
    abort();
  previous = callback;
  for (int i = 0; i < 32; ++i) {
    if (callback(data) != 4 || !pthread_equal(origin, pthread_self()))
      abort();
  }
  return 4;
}
uint64_t invoke_zero(uint64_t (*callback)(const uint8_t *), const uint8_t *data) {
  if (!data)
    abort();
  for (int i = 0; i < 32; ++i)
    if (callback(NULL) != 0)
      abort();
  return 0;
}
uint64_t invoke_text(uint64_t (*callback)(const char *), const char *data) {
  pthread_t origin = pthread_self();
  if (!data || strlen(data) != 9)
    abort();
  for (int i = 0; i < 32; ++i) {
    if (callback(data) != 9 || !pthread_equal(origin, pthread_self()))
      abort();
  }
  return 9;
}
void touch(void) {
  atomic_fetch_add(&touches, 1);
}
uint64_t count(void) {
  return atomic_load(&touches);
}
int8_t signed8(int8_t value) {
  return value;
}
uint64_t fire_late(void) {
  static const uint8_t bytes[4] = {65, 255, 0, 0};
  return previous(bytes);
}
static void *wrong_thread(void *raw) {
  uint64_t (**callback)(const uint8_t *) = raw;
  static const uint8_t bytes[4] = {65, 255, 0, 0};
  (*callback)(bytes);
  abort();
}
uint64_t bad_bytes(uint64_t (*callback)(const uint8_t *), int32_t kind) {
  if (kind == 1)
    return callback(NULL);
  pthread_t thread;
  if (pthread_create(&thread, NULL, wrong_thread, &callback))
    abort();
  if (pthread_join(thread, NULL))
    abort();
  abort();
}
uint64_t bad_text(uint64_t (*callback)(const char *), int32_t kind) {
  static const char invalid[] = {(char)255, 0};
  return callback(kind == 3 ? invalid : NULL);
}
