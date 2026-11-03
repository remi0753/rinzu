#ifndef RINZU_RESOURCE_COUNTERS_H
#define RINZU_RESOURCE_COUNTERS_H
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* Private image-local observation. Enable before Domain/work creation.
 * Values never decide admission, GC roots, callback reclamation or scheduling. */
enum {
  RINZU_RESOURCE_MANAGED,
  RINZU_RESOURCE_MANAGED_OBJECTS,
  RINZU_RESOURCE_NATIVE_INPUT,
  RINZU_RESOURCE_TASK,
  RINZU_RESOURCE_FUTURE,
  RINZU_RESOURCE_PENDING_OPERATION,
  RINZU_RESOURCE_QUEUE,
  RINZU_RESOURCE_TRANSPORT,
  RINZU_RESOURCE_TRANSPORT_IN_FLIGHT,
  RINZU_RESOURCE_NATIVE_CALLS,
  RINZU_RESOURCE_NATIVE_COPY,
  RINZU_RESOURCE_REGISTRATION,
  RINZU_RESOURCE_RUNTIME_CONTROL,
  RINZU_RESOURCE_COUNT
};
typedef struct {
  uint64_t live, allocated, released, peak;
} rinzu_resource_counter_t;
static _Atomic int rinzu_resource_enabled;
static pthread_mutex_t rinzu_resource_mutex = PTHREAD_MUTEX_INITIALIZER;
static rinzu_resource_counter_t rinzu_resource_counters[RINZU_RESOURCE_COUNT];
static inline void rinzu_resource_lock(void) {
  if (pthread_mutex_lock(&rinzu_resource_mutex))
    _Exit(86);
}
static inline void rinzu_resource_unlock(void) {
  if (pthread_mutex_unlock(&rinzu_resource_mutex))
    _Exit(86);
}
static inline void rinzu_resource_acquire(unsigned kind, uint64_t amount) {
  if (!atomic_load_explicit(&rinzu_resource_enabled, memory_order_relaxed))
    return;
  if (kind >= RINZU_RESOURCE_COUNT)
    _Exit(86);
  rinzu_resource_lock();
  rinzu_resource_counter_t *counter = &rinzu_resource_counters[kind];
  if (counter->live > UINT64_MAX - amount || counter->allocated > UINT64_MAX - amount)
    _Exit(71);
  counter->live += amount;
  counter->allocated += amount;
  if (counter->live > counter->peak)
    counter->peak = counter->live;
  rinzu_resource_unlock();
}
static inline void rinzu_resource_release(unsigned kind, uint64_t amount) {
  if (!atomic_load_explicit(&rinzu_resource_enabled, memory_order_relaxed))
    return;
  if (kind >= RINZU_RESOURCE_COUNT)
    _Exit(86);
  rinzu_resource_lock();
  rinzu_resource_counter_t *counter = &rinzu_resource_counters[kind];
  if (counter->live < amount)
    _Exit(86);
  if (counter->released > UINT64_MAX - amount)
    _Exit(71);
  counter->live -= amount;
  counter->released += amount;
  rinzu_resource_unlock();
}
static inline void rinzu_resource_copy(uint64_t amount) {
  if (!atomic_load_explicit(&rinzu_resource_enabled, memory_order_relaxed))
    return;
  rinzu_resource_lock();
  rinzu_resource_counter_t *counter = &rinzu_resource_counters[RINZU_RESOURCE_NATIVE_COPY];
  if (counter->allocated > UINT64_MAX - amount)
    _Exit(71);
  counter->allocated += amount;
  rinzu_resource_unlock();
}
typedef struct rinzu_resource_site {
  unsigned fallback;
  uint64_t file, start, end, bytes;
  const char *symbol;
  struct rinzu_resource_site *next;
} rinzu_resource_site_t;
static rinzu_resource_site_t *rinzu_resource_sites;
/* Copied at quiescent run end; the sampler never dereferences a queue. */
static int rinzu_resource_foreign_known;
static uint64_t rinzu_resource_foreign_live, rinzu_resource_foreign_bytes;
static uint64_t rinzu_resource_foreign_created, rinzu_resource_foreign_retired;
static inline void rinzu_resource_foreign_end(uint64_t live, uint64_t bytes, uint64_t created,
                                              uint64_t retired) {
  if (!atomic_load_explicit(&rinzu_resource_enabled, memory_order_relaxed))
    return;
  rinzu_resource_foreign_live = live;
  rinzu_resource_foreign_bytes = bytes;
  rinzu_resource_foreign_created = created;
  rinzu_resource_foreign_retired = retired;
  rinzu_resource_foreign_known = 1;
}
/* Generated symbols are immutable strings pinned through image report completion. */
static inline void rinzu_resource_return_site(unsigned fallback, size_t bytes, uint64_t file,
                                              uint64_t start, uint64_t end, const char *symbol) {
  if (!atomic_load_explicit(&rinzu_resource_enabled, memory_order_relaxed) || !file || !symbol)
    return;
  rinzu_resource_lock();
  rinzu_resource_site_t *site = rinzu_resource_sites;
  for (; site; site = site->next)
    if (site->fallback == fallback && site->file == file && site->start == start &&
        site->end == end && !strcmp(site->symbol, symbol))
      break;
  if (!site) {
    site = calloc(1, sizeof(*site));
    if (!site)
      _Exit(71);
    *site = (rinzu_resource_site_t){fallback, file, start, end, 0, symbol, rinzu_resource_sites};
    rinzu_resource_sites = site;
  }
  if (site->bytes > UINT64_MAX - bytes)
    _Exit(71);
  site->bytes += bytes;
  rinzu_resource_unlock();
}
#endif
