#include "api.h"
#include <assert.h>
#include <pthread.h>
#include <stdlib.h>
#include <time.h>
struct Witness {
  pthread_t owner;
  struct Witness *next;
};
static pthread_mutex_t mutex = PTHREAD_MUTEX_INITIALIZER;
static struct Witness *values;
static void require_owner(struct Witness *value) {
  assert(value && pthread_equal(value->owner, pthread_self()));
}
struct Witness *thread_create(int32_t main) {
  assert(main == 0 || main == 1 || main == 2);
  if (main != 2)
    assert(!!pthread_main_np() == main);
  struct Witness *value = calloc(1, sizeof(*value));
  if (!value)
    _Exit(71);
  value->owner = pthread_self();
  assert(!pthread_mutex_lock(&mutex));
  value->next = values;
  values = value;
  assert(!pthread_mutex_unlock(&mutex));
  return value;
}
int32_t thread_ping(struct Witness *value) {
  require_owner(value);
  return 41;
}
int32_t thread_invoke(struct Witness *value, int32_t (*body)(struct Witness *, int32_t),
                      int32_t depth) {
  require_owner(value);
  assert(body && depth >= 0 && depth <= 99);
  int32_t result = body(value, depth);
  require_owner(value);
  return result;
}
void thread_destroy(struct Witness *value) {
  require_owner(value);
  assert(!pthread_mutex_lock(&mutex));
  struct Witness **cursor = &values;
  while (*cursor && *cursor != value)
    cursor = &(*cursor)->next;
  assert(*cursor);
  *cursor = value->next;
  assert(!pthread_mutex_unlock(&mutex));
  free(value);
}
int32_t thread_live(void) {
  assert(!pthread_mutex_lock(&mutex));
  int32_t count = 0;
  for (struct Witness *value = values; value; value = value->next)
    count++;
  assert(!pthread_mutex_unlock(&mutex));
  return count;
}
int32_t thread_clear(void) {
  assert(!pthread_mutex_lock(&mutex));
  struct Witness *retained = values;
  values = NULL;
  assert(!pthread_mutex_unlock(&mutex));
  int32_t count = 0;
  while (retained) {
    struct Witness *next = retained->next;
    free(retained);
    retained = next;
    count++;
  }
  return count;
}
static pthread_cond_t condition = PTHREAD_COND_INITIALIZER;
static int blocked, released;
static struct timespec deadline(void) {
  struct timespec limit;
  assert(!clock_gettime(CLOCK_REALTIME, &limit));
  limit.tv_sec += 30;
  return limit;
}
int32_t thread_block(struct Witness *value, int32_t (*body)(struct Witness *, int32_t)) {
  require_owner(value);
  assert(!pthread_main_np() && body);
  struct timespec limit = deadline();
  assert(!pthread_mutex_lock(&mutex));
  assert(!blocked && !released);
  blocked = 1;
  assert(!pthread_cond_broadcast(&condition));
  while (!released)
    assert(!pthread_cond_timedwait(&condition, &mutex, &limit));
  assert(blocked);
  blocked = 0;
  assert(!pthread_mutex_unlock(&mutex));
  int32_t result = body(value, 0);
  require_owner(value);
  return result;
}
void thread_wait_blocked(void) {
  assert(pthread_main_np());
  struct timespec limit = deadline();
  assert(!pthread_mutex_lock(&mutex));
  while (!blocked)
    assert(!pthread_cond_timedwait(&condition, &mutex, &limit));
  assert(!released);
  assert(!pthread_mutex_unlock(&mutex));
}
int32_t thread_is_blocked(void) {
  assert(pthread_main_np());
  assert(!pthread_mutex_lock(&mutex));
  int32_t active = blocked && !released;
  assert(!pthread_mutex_unlock(&mutex));
  return active;
}
void thread_release_blocked(void) {
  assert(pthread_main_np());
  assert(!pthread_mutex_lock(&mutex));
  assert(blocked && !released);
  released = 1;
  assert(!pthread_cond_broadcast(&condition));
  assert(!pthread_mutex_unlock(&mutex));
}
