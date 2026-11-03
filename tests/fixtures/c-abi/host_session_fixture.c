#include "rinzu/runtime.h"

#include <assert.h>
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

struct host_state {
  rinzu_host_owner_turn_callback_t owner_turn;
  void *owner_context;
  uint64_t generation;
  atomic_bool active;
  atomic_bool posted;
  atomic_bool hold_post;
  atomic_bool in_post;
  atomic_bool allow_post_return;
  atomic_size_t deactivate_count;
};

struct enqueue_args {
  rinzu_host_session_t *session;
  atomic_size_t *accepted;
  atomic_size_t *completed_producers;
  atomic_size_t *work_count;
  size_t deliveries;
};

struct leave_args {
  rinzu_host_session_t *session;
  rinzu_status_t status;
};

struct release_args {
  struct host_state *state;
  size_t yields;
};

static uint64_t next_random(uint64_t *state) {
  uint64_t value = *state;
  value ^= value << 13;
  value ^= value >> 7;
  value ^= value << 17;
  *state = value;
  return value;
}

static rinzu_status_t prepare_host(void *opaque, rinzu_host_owner_turn_callback_t owner_turn,
                                   void *owner_context, uint64_t generation,
                                   void **out_ready_token) {
  struct host_state *state = opaque;
  assert(state != NULL);
  assert(owner_turn != NULL);
  assert(out_ready_token != NULL);
  state->owner_turn = owner_turn;
  state->owner_context = owner_context;
  state->generation = generation;
  atomic_store_explicit(&state->posted, false, memory_order_relaxed);
  atomic_store_explicit(&state->active, true, memory_order_release);
  *out_ready_token = state;
  return RINZU_STATUS_OK;
}

static int32_t post_owner_turn(void *opaque, void *ready_token, uint64_t generation) {
  struct host_state *state = opaque;
  if (state != ready_token || generation != state->generation ||
      !atomic_load_explicit(&state->active, memory_order_acquire)) {
    return RINZU_HOST_POST_CLOSED;
  }
  if (atomic_load_explicit(&state->hold_post, memory_order_acquire)) {
    atomic_store_explicit(&state->in_post, true, memory_order_release);
    while (!atomic_load_explicit(&state->allow_post_return, memory_order_acquire)) {
      sched_yield();
    }
  }
  atomic_store_explicit(&state->posted, true, memory_order_release);
  return RINZU_HOST_POST_ACCEPTED;
}

static void deactivate_host(void *opaque, void *ready_token, uint64_t generation) {
  struct host_state *state = opaque;
  assert(state == ready_token);
  assert(generation == state->generation);
  bool was_active = atomic_exchange_explicit(&state->active, false, memory_order_acq_rel);
  assert(was_active);
  atomic_fetch_add_explicit(&state->deactivate_count, 1, memory_order_acq_rel);
}

static rinzu_host_loop_profile_v1_t profile_for(struct host_state *state) {
  return (rinzu_host_loop_profile_v1_t){
      .struct_size = sizeof(rinzu_host_loop_profile_v1_t),
      .abi_version = RINZU_ABI_VERSION,
      .context = state,
      .prepare = prepare_host,
      .post_owner_turn = post_owner_turn,
      .deactivate = deactivate_host,
      .nested_delivery = RINZU_HOST_NESTED_UNSUPPORTED,
      .return_policy = RINZU_HOST_RETURN_RESUME_MANAGED,
      .turn_budget = 7,
      .reserved = 0,
  };
}

static void run_work(void *opaque) {
  atomic_size_t *count = opaque;
  atomic_fetch_add_explicit(count, 1, memory_order_acq_rel);
}

static void drain_one_post(struct host_state *state) {
  if (atomic_exchange_explicit(&state->posted, false, memory_order_acq_rel)) {
    state->owner_turn(state->owner_context, state->generation);
  }
}

static void *enqueue_many(void *opaque) {
  struct enqueue_args *args = opaque;
  for (size_t index = 0; index < args->deliveries; index++) {
    rinzu_status_t status = rinzu_host_session_enqueue(args->session, run_work, args->work_count);
    if (status == RINZU_STATUS_OK) {
      atomic_fetch_add_explicit(args->accepted, 1, memory_order_acq_rel);
    } else {
      assert(status == RINZU_STATUS_QUEUE_FULL);
    }
    sched_yield();
  }
  atomic_fetch_add_explicit(args->completed_producers, 1, memory_order_release);
  return NULL;
}

static void *wrong_thread_leave(void *opaque) {
  struct leave_args *args = opaque;
  args->status = rinzu_host_session_leave(args->session);
  return NULL;
}

static void exercise_delivery(rinzu_runtime_t *runtime) {
  enum { PRODUCERS = 6, DELIVERIES = 500 };
  struct host_state state = {0};
  atomic_init(&state.active, false);
  atomic_init(&state.posted, false);
  atomic_init(&state.hold_post, false);
  atomic_init(&state.in_post, false);
  atomic_init(&state.allow_post_return, true);
  atomic_init(&state.deactivate_count, 0);
  rinzu_host_loop_profile_v1_t profile = profile_for(&state);
  rinzu_host_session_t *session = NULL;
  assert(rinzu_host_session_create(runtime, &profile, &session) == RINZU_STATUS_OK);
  assert(session != NULL);
  uint64_t generation = 0;
  assert(rinzu_host_session_enter(session, &generation) == RINZU_STATUS_OK);
  assert(generation == 1);
  assert(rinzu_host_session_enqueue(session, NULL, NULL) == RINZU_STATUS_INVALID_ARGUMENT);

  struct leave_args bad_leave = {.session = session, .status = -1};
  pthread_t bad_thread;
  assert(pthread_create(&bad_thread, NULL, wrong_thread_leave, &bad_leave) == 0);
  assert(pthread_join(bad_thread, NULL) == 0);
  assert(bad_leave.status == RINZU_STATUS_WRONG_THREAD);

  atomic_size_t accepted;
  atomic_size_t completed_producers;
  atomic_size_t work_count;
  atomic_init(&accepted, 0);
  atomic_init(&completed_producers, 0);
  atomic_init(&work_count, 0);
  pthread_t producers[PRODUCERS];
  struct enqueue_args arguments[PRODUCERS];
  for (size_t index = 0; index < PRODUCERS; index++) {
    arguments[index] = (struct enqueue_args){
        .session = session,
        .accepted = &accepted,
        .completed_producers = &completed_producers,
        .work_count = &work_count,
        .deliveries = DELIVERIES,
    };
    assert(pthread_create(&producers[index], NULL, enqueue_many, &arguments[index]) == 0);
  }

  while (atomic_load_explicit(&completed_producers, memory_order_acquire) < PRODUCERS ||
         atomic_load_explicit(&work_count, memory_order_acquire) <
             atomic_load_explicit(&accepted, memory_order_acquire)) {
    drain_one_post(&state);
    sched_yield();
  }
  for (size_t index = 0; index < PRODUCERS; index++) {
    assert(pthread_join(producers[index], NULL) == 0);
  }
  while (atomic_load_explicit(&work_count, memory_order_acquire) <
         atomic_load_explicit(&accepted, memory_order_acquire)) {
    drain_one_post(&state);
  }

  assert(rinzu_host_session_leave(session) == RINZU_STATUS_OK);
  assert(atomic_load_explicit(&state.deactivate_count, memory_order_acquire) == 1);
  size_t delivered = atomic_load_explicit(&work_count, memory_order_acquire);
  state.owner_turn(state.owner_context, generation); /* late delivery */
  assert(atomic_load_explicit(&work_count, memory_order_acquire) == delivered);
  assert(rinzu_host_session_destroy(session) == RINZU_STATUS_OK);
}

static void *enqueue_once(void *opaque) {
  struct enqueue_args *args = opaque;
  rinzu_status_t status = rinzu_host_session_enqueue(args->session, run_work, args->work_count);
  assert(status == RINZU_STATUS_OK);
  return NULL;
}

static void *release_blocked_post(void *opaque) {
  struct release_args *args = opaque;
  struct host_state *state = args->state;
  while (!atomic_load_explicit(&state->in_post, memory_order_acquire)) {
    sched_yield();
  }
  for (size_t index = 0; index < args->yields; index++) {
    sched_yield();
  }
  atomic_store_explicit(&state->allow_post_return, true, memory_order_release);
  return NULL;
}

static void exercise_unregister_race(rinzu_runtime_t *runtime, size_t iterations, uint64_t seed) {
  uint64_t random = seed == 0 ? UINT64_C(0x9e3779b97f4a7c15) : seed;
  for (size_t iteration = 0; iteration < iterations; iteration++) {
    struct host_state state = {0};
    atomic_init(&state.active, false);
    atomic_init(&state.posted, false);
    atomic_init(&state.hold_post, true);
    atomic_init(&state.in_post, false);
    atomic_init(&state.allow_post_return, false);
    atomic_init(&state.deactivate_count, 0);
    rinzu_host_loop_profile_v1_t profile = profile_for(&state);
    rinzu_host_session_t *session = NULL;
    assert(rinzu_host_session_create(runtime, &profile, &session) == RINZU_STATUS_OK);
    uint64_t generation = 0;
    assert(rinzu_host_session_enter(session, &generation) == RINZU_STATUS_OK);

    atomic_size_t work_count;
    atomic_init(&work_count, 0);
    struct enqueue_args args = {
        .session = session,
        .work_count = &work_count,
    };
    pthread_t producer;
    pthread_t releaser;
    struct release_args release = {
        .state = &state,
        .yields = (size_t)(next_random(&random) & UINT64_C(255)),
    };
    assert(pthread_create(&producer, NULL, enqueue_once, &args) == 0);
    assert(pthread_create(&releaser, NULL, release_blocked_post, &release) == 0);
    while (!atomic_load_explicit(&state.in_post, memory_order_acquire)) {
      sched_yield();
    }
    assert(rinzu_host_session_leave(session) == RINZU_STATUS_OK);
    assert(pthread_join(producer, NULL) == 0);
    assert(pthread_join(releaser, NULL) == 0);
    assert(atomic_load_explicit(&state.deactivate_count, memory_order_acquire) == 1);
    assert(atomic_load_explicit(&work_count, memory_order_acquire) == 0);
    state.owner_turn(state.owner_context, generation); /* stale */
    assert(rinzu_host_session_destroy(session) == RINZU_STATUS_OK);
  }
}

int main(void) {
  rinzu_runtime_options_t options;
  assert(rinzu_runtime_options_init(&options) == RINZU_STATUS_OK);
  rinzu_runtime_t *runtime = NULL;
  assert(rinzu_runtime_create(&options, &runtime) == RINZU_STATUS_OK);
  exercise_delivery(runtime);

  const char *configured = getenv("RINZU_STRESS_ITERATIONS");
  size_t iterations = configured == NULL ? 2000 : strtoull(configured, NULL, 10);
  const char *configured_seed = getenv("RINZU_STRESS_SEED");
  uint64_t seed =
      configured_seed == NULL ? UINT64_C(8241981735943431729) : strtoull(configured_seed, NULL, 10);
  assert(iterations > 0);
  exercise_unregister_race(runtime, iterations, seed);
  rinzu_runtime_destroy(runtime);
  printf("production Host Session fixture: ok (%zu unregister races, seed "
         "%llu)\n",
         iterations, (unsigned long long)seed);
  return 0;
}
