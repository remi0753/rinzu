#include <assert.h>
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>

enum registration_state {
  REG_OPEN = 0,
  REG_FAILED = 1,
  REG_CLOSING = 2,
  REG_CLOSED = 3,
};

struct registration {
  atomic_int state;
  atomic_uint generation;
  atomic_size_t unregister_count;
  atomic_size_t release_count;
  pthread_mutex_t mutex;
  size_t capacity;
  bool active;
  size_t queued;
};

struct producer_args {
  struct registration *registration;
  atomic_bool *start;
  atomic_size_t *completed;
  unsigned generation;
  size_t deliveries;
};

static void release_argument(struct registration *registration) {
  atomic_fetch_add_explicit(&registration->release_count, 1, memory_order_acq_rel);
}

static void unregister_once(struct registration *registration) {
  size_t expected = 0;
  (void)atomic_compare_exchange_strong_explicit(&registration->unregister_count, &expected, 1,
                                                memory_order_acq_rel, memory_order_acquire);
}

static void release_handlers_locked(struct registration *registration) {
  if (registration->active) {
    registration->active = false;
    release_argument(registration);
  }
  while (registration->queued > 0) {
    registration->queued--;
    release_argument(registration);
  }
}

static void deliver(struct registration *registration, unsigned generation) {
  if (generation != atomic_load_explicit(&registration->generation, memory_order_acquire) ||
      atomic_load_explicit(&registration->state, memory_order_acquire) != REG_OPEN) {
    release_argument(registration);
    return;
  }

  assert(pthread_mutex_lock(&registration->mutex) == 0);
  if (generation != atomic_load_explicit(&registration->generation, memory_order_acquire) ||
      atomic_load_explicit(&registration->state, memory_order_acquire) != REG_OPEN) {
    assert(pthread_mutex_unlock(&registration->mutex) == 0);
    release_argument(registration);
    return;
  }

  if (!registration->active) {
    registration->active = true;
    assert(pthread_mutex_unlock(&registration->mutex) == 0);
    return;
  }

  if (registration->queued < registration->capacity) {
    registration->queued++;
    assert(pthread_mutex_unlock(&registration->mutex) == 0);
    return;
  }

  int expected = REG_OPEN;
  bool winner = atomic_compare_exchange_strong_explicit(&registration->state, &expected, REG_FAILED,
                                                        memory_order_acq_rel, memory_order_acquire);
  release_argument(registration);
  release_handlers_locked(registration);
  assert(pthread_mutex_unlock(&registration->mutex) == 0);

  if (winner) {
    atomic_fetch_add_explicit(&registration->generation, 1, memory_order_acq_rel);
    unregister_once(registration);
  }
}

static void complete_active(struct registration *registration) {
  assert(pthread_mutex_lock(&registration->mutex) == 0);
  if (registration->active) {
    registration->active = false;
    release_argument(registration);
  }
  if (atomic_load_explicit(&registration->state, memory_order_acquire) == REG_OPEN &&
      registration->queued > 0) {
    registration->queued--;
    registration->active = true;
  }
  assert(pthread_mutex_unlock(&registration->mutex) == 0);
}

static void close_registration(struct registration *registration) {
  int previous = atomic_exchange_explicit(&registration->state, REG_CLOSING, memory_order_acq_rel);
  if (previous == REG_CLOSED) {
    return;
  }

  atomic_fetch_add_explicit(&registration->generation, 1, memory_order_acq_rel);
  unregister_once(registration);
  assert(pthread_mutex_lock(&registration->mutex) == 0);
  release_handlers_locked(registration);
  assert(pthread_mutex_unlock(&registration->mutex) == 0);
  atomic_store_explicit(&registration->state, REG_CLOSED, memory_order_release);
}

static void *producer_main(void *opaque) {
  struct producer_args *args = opaque;
  while (!atomic_load_explicit(args->start, memory_order_acquire)) {
    sched_yield();
  }
  for (size_t index = 0; index < args->deliveries; index++) {
    deliver(args->registration, args->generation);
    sched_yield();
  }
  atomic_fetch_add_explicit(args->completed, 1, memory_order_release);
  return NULL;
}

int main(void) {
  enum { PRODUCERS = 8, DELIVERIES = 100 };
  struct registration registration = {
      .state = ATOMIC_VAR_INIT(REG_OPEN),
      .generation = ATOMIC_VAR_INIT(1),
      .unregister_count = ATOMIC_VAR_INIT(0),
      .release_count = ATOMIC_VAR_INIT(0),
      .mutex = PTHREAD_MUTEX_INITIALIZER,
      .capacity = 4,
      .active = false,
      .queued = 0,
  };
  atomic_bool start = ATOMIC_VAR_INIT(false);
  atomic_size_t completed = ATOMIC_VAR_INIT(0);
  pthread_t threads[PRODUCERS];
  struct producer_args args[PRODUCERS];

  for (size_t index = 0; index < PRODUCERS; index++) {
    args[index] = (struct producer_args){
        .registration = &registration,
        .start = &start,
        .completed = &completed,
        .generation = 1,
        .deliveries = DELIVERIES,
    };
    assert(pthread_create(&threads[index], NULL, producer_main, &args[index]) == 0);
  }

  atomic_store_explicit(&start, true, memory_order_release);
  while (atomic_load_explicit(&completed, memory_order_acquire) < PRODUCERS) {
    complete_active(&registration);
    sched_yield();
  }
  close_registration(&registration);

  for (size_t index = 0; index < PRODUCERS; index++) {
    assert(pthread_join(threads[index], NULL) == 0);
  }

  deliver(&registration, 1); /* late callback */

  assert(atomic_load_explicit(&registration.state, memory_order_acquire) == REG_CLOSED);
  assert(atomic_load_explicit(&registration.unregister_count, memory_order_acquire) == 1);
  assert(atomic_load_explicit(&registration.release_count, memory_order_acquire) ==
         (size_t)(PRODUCERS * DELIVERIES + 1));
  assert(!registration.active);
  assert(registration.queued == 0);
  assert(pthread_mutex_destroy(&registration.mutex) == 0);

  puts("callback fixture: ok");
  return 0;
}
