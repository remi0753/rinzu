#include <assert.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>

/* Target storage is intentionally retained by this safe fixture after explicit
   release; aliases can be observed without a real use-after-free. */
struct Object {
  int64_t value;
  pthread_t owner;
  int alive;
  struct Object *next;
};
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static struct Object *objects;
static _Atomic uint64_t counters[6];
static void entry_marker(void) {
  const char *path = getenv("RINZU_TEST_OPAQUE_ENTRY");
  if (!path)
    return;
  int fd = open(path, O_WRONLY | O_CREAT | O_APPEND, 0600);
  if (fd < 0 || write(fd, "E", 1) != 1 || close(fd))
    _Exit(72);
}
void *object_create(int64_t value) {
  entry_marker();
  atomic_fetch_add(&counters[4], 1);
  struct Object *object = calloc(1, sizeof(*object));
  if (!object)
    _Exit(71);
  object->value = value;
  object->owner = pthread_self();
  object->alive = 1;
  assert(!pthread_mutex_lock(&lock));
  object->next = objects;
  objects = object;
  assert(!pthread_mutex_unlock(&lock));
  atomic_fetch_add(&counters[0], 1);
  atomic_fetch_add(&counters[3], 1);
  return object;
}
void *object_absent(void) {
  entry_marker();
  atomic_fetch_add(&counters[4], 1);
  return NULL;
}
static struct Object *checked(void *pointer) {
  struct Object *object = pointer;
  assert(object && object->alive && pthread_equal(object->owner, pthread_self()));
  return object;
}
void *object_lookup(void *pointer, uint64_t (*callback)(const unsigned char *)) {
  struct Object *object = checked(pointer);
  const unsigned char bytes[4] = {0, 255, 7, 0};
  for (unsigned round = 0; round < 32; ++round)
    assert(callback(bytes) == 4);
  assert(checked(pointer) == object);
  return pointer;
}
void *object_lookup_missing(void *pointer, uint64_t (*callback)(const unsigned char *)) {
  (void)object_lookup(pointer, callback);
  return NULL;
}
int64_t object_read(void *pointer) {
  return checked(pointer)->value;
}
void object_retain(void *pointer) {
  (void)checked(pointer);
  atomic_fetch_add(&counters[2], 1);
}
void object_release(void *pointer) {
  struct Object *object = checked(pointer);
  object->alive = 0;
  atomic_fetch_add(&counters[1], 1);
  atomic_fetch_sub(&counters[3], 1);
}
int64_t object_alias_observed(void *pointer) {
  assert(!pthread_mutex_lock(&lock));
  int64_t found = 0;
  for (struct Object *object = objects; object; object = object->next)
    if (object == pointer)
      found = 1;
  assert(!pthread_mutex_unlock(&lock));
  atomic_fetch_add(&counters[5], 1);
  return found;
}
uint64_t object_stats(int32_t kind) {
  assert(kind >= 0 && kind < 6);
  return atomic_load(&counters[kind]);
}
void object_fixture_cleanup(void) {
  assert(!pthread_mutex_lock(&lock));
  while (objects) {
    struct Object *object = objects;
    objects = object->next;
    free(object);
  }
  for (size_t index = 0; index < 6; ++index)
    atomic_store(&counters[index], 0);
  assert(!pthread_mutex_unlock(&lock));
}
